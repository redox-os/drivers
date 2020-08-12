#![feature(get_mut_unchecked, llvm_asm, try_reserve)]

use std::collections::{BTreeMap, BTreeSet};
use std::convert::TryInto;
use std::fs::{metadata, read_dir, File};
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::{Arc, Mutex, RwLock};
use std::{env, mem, process, slice, thread};

use syscall::data::Packet;
use syscall::error::Error;
use syscall::error::EINTR;
use syscall::flag::{O_CREAT, O_EXCL, O_RDWR};
use syscall::scheme::Scheme as _;

use redox_iou::executor::Executor;
use redox_iou::instance::ConsumerInstanceBuilder;
use redox_iou::reactor::{ReactorBuilder, SubmissionContext};

use redox_log::{OutputBuilder, RedoxLogger};

mod config;

#[allow(dead_code)]
mod driver_interface;

mod pci;
mod pcie;
mod scheme;

use crate::config::Config;
use crate::driver_interface::{PciAddress32, PcidServerResponseError, SetCapabilityInfo};
use crate::pci::cap::{self as pcicap, Capability as PciCapability};
use crate::pci::{
    CfgAccess, Pci, PciBar, PciBus, PciClass, PciDev, PciFunc, PciHeader, PciHeaderError, PciIter,
};
use crate::pcie::cap::{self as pciecap, Capability as PcieCapability};
use crate::pcie::Pcie;
use crate::scheme::PcidScheme;

// TODO: Move this helper trait to redox-log.
trait ResultExt
where
    Self: Sized,
{
    fn and_log_err_as_error(self, msg: &str) -> Self;
    fn and_log_err_as_warn(self, msg: &str) -> Self;
}

impl<T, E> ResultExt for core::result::Result<T, E>
where
    E: core::fmt::Display,
{
    fn and_log_err_as_error(self, msg: &str) -> Self {
        self.map_err(|err| {
            log::error!("{}: {}", msg, err);
            err
        })
    }
    fn and_log_err_as_warn(self, msg: &str) -> Self {
        self.map_err(|err| {
            log::warn!("{}: {}", msg, err);
            err
        })
    }
}

#[derive(Debug)]
pub struct Func {
    header: PciHeader,
    bar_sizes: [u64; 6],

    pci_capabilities: Vec<(u8, PciCapability)>,
    pcie_capabilities: Vec<(u16, PcieCapability)>,
}

pub struct DeviceTree {
    pub functions: BTreeMap<PciAddress32, Arc<RwLock<Func>>>,
    pub devices: BTreeSet<(u16, u8, u8)>,
    pub busses: BTreeSet<(u16, u8)>,
    pub uses_seg_groups: bool,
}

pub struct DriverHandler {
    config: config::DriverConfig,
    bus_num: u8,
    dev_num: u8,
    func_num: u8,
    func: Arc<RwLock<Func>>,

    state: Arc<State>,
}
fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(
    pci: &dyn CfgAccess,
    bus_num: u8,
    dev_num: u8,
    func_num: u8,
    function: F,
) -> T {
    let bus = PciBus { pci, num: bus_num };
    let dev = PciDev {
        bus: &bus,
        num: dev_num,
    };
    let func = PciFunc {
        dev: &dev,
        num: func_num,
    };
    function(&func)
}
fn set_capability_info(func: &mut Func, info_to_set: &SetCapabilityInfo, addr: PciAddress32, cfg_access: &dyn CfgAccess) -> Result<(), PcidServerResponseError> {
    use crate::driver_interface::*;

    let mut msi_enabled = false;
    let mut msix_enabled = false;

    for cap in func.pci_capabilities.iter() {
        match cap {
            &(_, PciCapability::Msi(ref cap)) => msi_enabled = cap.enabled(),
            &(_, PciCapability::MsiX(ref cap)) => msix_enabled = cap.msix_enabled(),
            _ => (),
        }
    }

    match *info_to_set {
        SetCapabilityInfo::Msi(ref info_to_set) => {
            if let Some((offset, info)) = func
                .pci_capabilities
                .iter_mut()
                .find_map(|(offset, capability)| {
                    Some((*offset, capability.as_msi_mut()?))
                })
            {
                let info_to_set_flags =
                    match MsiSetCapabilityInfoFlags::from_bits(info_to_set.flags) {
                        Some(f) => f,
                        None => {
                            return Err(
                                PcidServerResponseError::InvalidBitPattern,
                            )
                        }
                    };

                if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::ENABLED)
                    && info_to_set.enabled == true as u8
                {
                    if msix_enabled {
                        log::error!("Client trying to enable MSI while MSI-X is already enabled.");
                        return Err(
                            PcidServerResponseError::InvalidBitPattern,
                        );
                    }
                }

                if info_to_set_flags
                    .contains(MsiSetCapabilityInfoFlags::MULTI_MESSAGE_ENABLE)
                {
                    let mme = info_to_set.multi_message_enable;
                    if info.multi_message_capable() < mme || mme > 0b101 {
                        return Err(
                            PcidServerResponseError::InvalidBitPattern,
                        );
                    }
                    info.set_multi_message_enable(mme);
                }
                if info_to_set_flags
                    .contains(MsiSetCapabilityInfoFlags::MESSAGE_ADDRESS)
                {
                    let message_addr = info_to_set.message_address;
                    if message_addr & 0b11 != 0 {
                        return Err(
                            PcidServerResponseError::InvalidBitPattern,
                        );
                    }
                    info.set_message_address(message_addr);
                }
                if info_to_set_flags
                    .contains(MsiSetCapabilityInfoFlags::MESSAGE_UPPER_ADDRESS)
                {
                    let message_addr_upper = info_to_set.message_upper_address;
                    info.set_message_upper_address(message_addr_upper);
                }
                if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MESSAGE_DATA) {
                    let message_data = info_to_set.message_data;
                    if message_data & ((1 << info.multi_message_enable()) - 1) != 0 {
                        return Err(
                            PcidServerResponseError::InvalidBitPattern,
                        );
                    }
                    info.set_message_data(message_data);
                }
                if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MASK_BITS) {
                    let mask_bits = info_to_set.mask_bits;
                    info.set_mask_bits(mask_bits);
                }
                unsafe {
                    with_pci_func_raw(
                        cfg_access,
                        addr.bus(),
                        addr.device(),
                        addr.function(),
                        |func| {
                            info.write_all(func, offset);
                        },
                    );
                }
                Ok(())
            } else {
                return Err(
                    PcidServerResponseError::NonexistentCapability(CapabilityType::Msi),
                );
            }
        }
        SetCapabilityInfo::MsiX(MsiXSetCapabilityInfo {
            function_mask,
            enabled,
            flags,
        }) => {
            if let Some((offset, info)) = func
                .pci_capabilities
                .iter_mut()
                .find_map(|(offset, capability)| {
                    Some((*offset, capability.as_msix_mut()?))
                })
            {
                let mut write = false;

                let flags = match MsiXSetCapabilityInfoFlags::from_bits(flags) {
                    Some(f) => f,
                    None => return Err(
                        driver_interface::PcidServerResponseError::InvalidBitPattern,
                    ),
                };
                if flags.contains(MsiXSetCapabilityInfoFlags::ENABLED) {
                    if msi_enabled {
                        log::error!("Client trying to enable MSI-X while MSI is already enabled.");
                        return Err(
                            PcidServerResponseError::InvalidBitPattern,
                        );
                    }
                    info.set_msix_enabled(enabled == true as u8);
                    write = true;
                }
                if flags.contains(MsiXSetCapabilityInfoFlags::FUNCTION_MASK) {
                    info.set_function_mask(function_mask == true as u8);
                    write = true;
                }
                if write {
                    unsafe {
                        with_pci_func_raw(
                            cfg_access,
                            addr.bus(),
                            addr.device(),
                            addr.function(),
                            |func| {
                                info.write_a(func, offset);
                            },
                        );
                    }
                }
                log::info!("UPDATED MSI-X CONFIG TO {:?}", info);
                Ok(())
            } else {
                return Err(
                    PcidServerResponseError::NonexistentCapability(
                        CapabilityType::MsiX,
                    ),
                );
            }
        }
    }
}
impl DriverHandler {
    fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(&self, function: F) -> T {
        with_pci_func_raw(
            self.state.preferred_cfg_access(),
            self.bus_num,
            self.dev_num,
            self.func_num,
            function,
        )
    }

    fn respond(
        &mut self,
        request: driver_interface::PcidClientRequest,
        args: &driver_interface::SubdriverArguments,
    ) -> driver_interface::PcidClientResponse {
        use driver_interface::*;

        match request {
            PcidClientRequest::RequestConfig => PcidClientResponse::Config(args.clone()),
            PcidClientRequest::GetCapabilities => {
                let func = self.func.read().unwrap();

                PcidClientResponse::AllCapabilities(
                    func.pci_capabilities
                        .iter()
                        .map(|(_, capability)| Capability::Pci(capability.clone()))
                        .chain(
                            func.pcie_capabilities
                                .iter()
                                .map(|(_, capability)| Capability::Pcie(capability.clone())),
                        )
                        .collect(),
                )
            }
            PcidClientRequest::GetCapability(ty) => PcidClientResponse::Capability(match ty {
                CapabilityType::Msi => self
                    .func
                    .read()
                    .unwrap()
                    .pci_capabilities
                    .iter()
                    .find_map(|(_, capability)| capability.as_msi().copied())
                    .map(PciCapability::Msi)
                    .map(Capability::Pci),
                CapabilityType::MsiX => self
                    .func
                    .read()
                    .unwrap()
                    .pci_capabilities
                    .iter()
                    .find_map(|(_, capability)| capability.as_msix().copied())
                    .map(PciCapability::MsiX)
                    .map(Capability::Pci),
                // TODO
                other => {
                    return PcidClientResponse::Error(
                        PcidServerResponseError::NonexistentCapability(other),
                    )
                }
            }),
            PcidClientRequest::SetCapability(ref info_to_set) => {
                let addr = PciAddress32::default()
                    .with_bus(self.bus_num)
                    .with_device(self.dev_num)
                    .with_function(self.func_num);
                match set_capability_info(&mut self.func.write().unwrap(), info_to_set, addr, self.state.preferred_cfg_access()) {
                    Ok(()) => return driver_interface::PcidClientResponse::SetCapability,
                    Err(err) => return driver_interface::PcidClientResponse::Error(err),
                }
            }
        }
    }
    fn handle_spawn(
        mut self,
        pcid_to_client_write: Option<usize>,
        pcid_from_client_read: Option<usize>,
        args: driver_interface::SubdriverArguments,
    ) {
        use driver_interface::*;

        if let (Some(pcid_to_client_fd), Some(pcid_from_client_fd)) =
            (pcid_to_client_write, pcid_from_client_read)
        {
            let mut pcid_to_client = unsafe { File::from_raw_fd(pcid_to_client_fd as RawFd) };
            let mut pcid_from_client = unsafe { File::from_raw_fd(pcid_from_client_fd as RawFd) };

            while let Ok(msg) = recv(&mut pcid_from_client) {
                let response = self.respond(msg, &args);
                send(&mut pcid_to_client, &response).unwrap();
            }
        }
    }
}

pub struct State {
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
    bare_commands: Mutex<Vec<process::Child>>,
    pci: Arc<Pci>,
    pcie: Option<Pcie>,
}
impl State {
    fn preferred_cfg_access(&self) -> &dyn CfgAccess {
        self.pcie
            .as_ref()
            .map_or(&*self.pci as &dyn CfgAccess, |pcie| pcie as &dyn CfgAccess)
    }
}

fn process_config(config: &Config, device_tree: &DeviceTree, state: &Arc<State>) {
    // TODO: Something faster than O(n^2)!

    for (&addr, func) in device_tree.functions.iter() {
        find_and_spawn_subdriver(addr, func, config, state, device_tree.uses_seg_groups);
    }
}

fn find_and_spawn_subdriver(
    addr: PciAddress32,
    func_arc: &Arc<RwLock<Func>>,
    config: &Config,
    state: &Arc<State>,
    uses_seg_groups: bool,
) {
    let func = func_arc.read().unwrap();
    let header = &func.header;

    for driver in config.drivers.iter() {
        if let Some(class) = driver.class {
            if class != u8::from(header.base().class) {
                continue;
            }
        }

        if let Some(subclass) = driver.subclass {
            if subclass != header.base().subclass {
                continue;
            }
        }

        if let Some(interface) = driver.interface {
            if interface != header.base().interface {
                continue;
            }
        }
        if let Some(ref ids) = driver.ids {
            let mut device_found = false;
            for (vendor, devices) in ids {
                let vendor_without_prefix = vendor.trim_start_matches("0x");
                let vendor = i64::from_str_radix(vendor_without_prefix, 16).unwrap() as u16;

                if vendor != header.base().vendor_id {
                    continue;
                }

                for device in devices {
                    if *device == header.base().device_id {
                        device_found = true;
                        break;
                    }
                }
            }
            if !device_found {
                continue;
            }
        } else {
            if let Some(vendor) = driver.vendor {
                if vendor != header.base().vendor_id {
                    continue;
                }
            }

            if let Some(device) = driver.device {
                if device != header.base().device_id {
                    continue;
                }
            }
        }
        if let Some(ref device_id_range) = driver.device_id_range {
            if header.base().device_id < device_id_range.start
                || device_id_range.end <= header.base().device_id
            {
                continue;
            }
        }

        let args = match &driver.command {
            Some(cmd) => cmd,
            None => continue,
        };

        let driver_name = args
            .iter()
            .map(|string| string.as_ref())
            .nth(0)
            .unwrap_or("[unknown]");

        log::info!(
            "PCI device capabilities for {}: {:?}",
            driver_name, func.pci_capabilities
        );
        log::info!(
            "PCI Express device capabilities for {}: {:?}",
            driver_name, func.pcie_capabilities
        );
        let mut args = args.into_iter();

        let raw_bars = match header {
            PciHeader::General(general) => general.bars,
            PciHeader::PciToPci(bridge) => {
                let mut bars = [0; 6];
                bars.copy_from_slice(&bridge.bars);
                bars
            }
        };
        let bars = match header.bars() {
            Ok(b) => b,
            Err(_) => continue,
        };

        let bar_sizes = &func.bar_sizes;
        let irq = header.legacy_interrupt_line();

        if let Some(program) = args.next() {
            let mut command = process::Command::new(program);

            let func_if = driver_interface::PciFunction {
                bars: raw_bars,
                bar_sizes: *bar_sizes,
                bus_num: addr.bus(),
                dev_num: addr.device(),
                func_num: addr.function(),
                devid: header.base().device_id,
                legacy_interrupt_line: irq,
                legacy_interrupt_pin: header.legacy_interrupt_pin().map_or(0, |pin| pin as u8),
                venid: header.base().vendor_id,
            };

            let subdriver_args = driver_interface::SubdriverArguments { func: func_if };

            let mut uses_deprecated_args = false;

            for arg in args {
                fn bar_str(bar: &Option<PciBar>) -> String {
                    bar.as_ref()
                        .map_or_else(|| "00000000".to_owned(), |bar| format!("{:>08X}", bar.address()))
                }

                // The following arguments are the only ones that are not warned about, if they are
                // passed
                let allowed = ["$BUS", "$DEV", "$FUNC", "$ADDRESS"];

                if !allowed.contains(&arg.as_str()) {
                    uses_deprecated_args = true;
                }

                // TODO: Deprecate this primitive form of message passing.
                let arg = match arg.as_str() {
                    "$BUS" => format!("{:>02X}", addr.bus()),
                    "$DEV" => format!("{:>02X}", addr.device()),
                    "$FUNC" => format!("{:>02X}", addr.function()),
                    "$ADDRESS" => addr.address(uses_seg_groups).to_string(),
                    "$NAME" => func_if.name(),
                    "$SCHEME_FRIENDLY_NAME" => func_if.scheme_friendly_name(),
                    "$BAR0" => bar_str(&bars[0]),
                    "$BAR1" => bar_str(&bars[1]),
                    "$BAR2" => bar_str(&bars[2]),
                    "$BAR3" => bar_str(&bars[3]),
                    "$BAR4" => bar_str(&bars[4]),
                    "$BAR5" => bar_str(&bars[5]),
                    "$BARSIZE0" => format!("{:>08X}", bar_sizes[0]),
                    "$BARSIZE1" => format!("{:>08X}", bar_sizes[1]),
                    "$BARSIZE2" => format!("{:>08X}", bar_sizes[2]),
                    "$BARSIZE3" => format!("{:>08X}", bar_sizes[3]),
                    "$BARSIZE4" => format!("{:>08X}", bar_sizes[4]),
                    "$BARSIZE5" => format!("{:>08X}", bar_sizes[5]),
                    "$IRQ" => format!("{}", irq),
                    "$VENID" => format!("{:>04X}", header.base().vendor_id),
                    "$DEVID" => format!("{:>04X}", header.base().device_id),
                    _ => arg.clone(),
                };
                command.arg(&arg);
            }

            log::info!("PCID SPAWN {:?}", command);

            let (pcid_to_client_write, pcid_from_client_read, envs, requires_handling) =
                if driver.use_channel.unwrap_or(false) {
                    let mut fds1 = [0usize; 2];
                    let mut fds2 = [0usize; 2];

                    syscall::pipe2(&mut fds1, 0).expect("pcid: failed to create pcid->client pipe");
                    syscall::pipe2(&mut fds2, 0).expect("pcid: failed to create client->pcid pipe");

                    let [pcid_to_client_read, pcid_to_client_write] = fds1;
                    let [pcid_from_client_read, pcid_from_client_write] = fds2;

                    (
                        Some(pcid_to_client_write),
                        Some(pcid_from_client_read),
                        vec![
                            ("PCID_TO_CLIENT_FD", format!("{}", pcid_to_client_read)),
                            ("PCID_FROM_CLIENT_FD", format!("{}", pcid_from_client_write)),
                        ],
                        true,
                    )
                } else {
                    (None, None, vec![], false)
                };

            if uses_deprecated_args {
                log::warn!("Driver {} uses the deprecated message passing, based on command line arguments.", program);
            }

            let mut child = match command.envs(envs).spawn() {
                Ok(child) => child,
                Err(err) => {
                    log::error!("failed to execute {:?}: {}", command, err);
                    return;
                }
            };
            if requires_handling {
                let driver_handler = DriverHandler {
                    bus_num: addr.bus(),
                    dev_num: addr.device(),
                    func_num: addr.function(),
                    config: driver.clone(),
                    state: Arc::clone(state),
                    func: Arc::clone(func_arc),
                };
                let thread = thread::Builder::new()
                    .name(format!("pcid `{}` handler", driver_name))
                    .spawn(move || {
                        driver_handler.handle_spawn(
                            pcid_to_client_write,
                            pcid_from_client_read,
                            subdriver_args,
                        );
                        match child.wait() {
                            Ok(status) => log::debug!("waited for {:?}, returned status {}", command, status),
                            Err(err) => log::error!("failed to wait for {:?}: {}", command, err),
                        }
                    }).expect("failed to spawn handler thread");
                state.threads.lock().unwrap().push(thread);
            } else {
                state.bare_commands.lock().unwrap().push(child);
            }
        }
    }
}

fn handle_parsed_header(
    state: Arc<State>,
    tree: &mut DeviceTree,
    bus_num: u8,
    dev_num: u8,
    func_num: u8,
    mut header: PciHeader,
) {
    let cfg_access = state.preferred_cfg_access();

    let raw_class: u8 = header.base().class.into();
    let mut string = format!(
        "PCI {:>02x}:{:>02x}.{:>01x} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
        bus_num,
        dev_num,
        func_num,
        header.base().vendor_id,
        header.base().device_id,
        raw_class,
        header.base().subclass,
        header.base().interface,
        header.base().revision,
        header.base().class,
    );
    log::debug!("HEADER: {:?}", &header);
    match header.base().class {
        PciClass::Legacy if header.base().subclass == 1 => string.push_str("  VGA CTL"),
        PciClass::Storage => match header.base().subclass {
            0x01 => {
                string.push_str(" IDE");
            }
            0x06 => {
                if header.base().interface == 0 {
                    string.push_str(" SATA VND");
                } else if header.base().interface == 1 {
                    string.push_str(" SATA AHCI");
                }
            }
            _ => (),
        },
        PciClass::SerialBus => match header.base().subclass {
            0x03 => match header.base().interface {
                0x00 => {
                    string.push_str(" UHCI");
                }
                0x10 => {
                    string.push_str(" OHCI");
                }
                0x20 => {
                    string.push_str(" EHCI");
                }
                0x30 => {
                    string.push_str(" XHCI");
                }
                _ => (),
            },
            _ => (),
        },
        _ => (),
    }

    // TODO: Should we disable these by default, and only enable them when the drivers allow us to
    // do that?

    // Enable bus mastering, memory space, and I/O space
    unsafe {
        let mut data = cfg_access.read(bus_num, dev_num, func_num, 0x04);
        log::debug!("sts_cmd dword before: {:#08x}", data);
        data |= 7;
        log::debug!("sts_cmd dword after: {:#08x}", data);
        cfg_access.write(bus_num, dev_num, func_num, 0x04, data);

        header.base_mut().command = (data & 0xFFFF) as u16;
        log::debug!("command: {:#04x}", header.base().command);
    }

    // TODO: Right now only the good old 8259 PIC is used, since AML is bloat and nobody has yet
    // managed to read the _PRT (PCI routing table) AML "Object". Hence, we're limited to IRQ 9,
    // 10, and 11, for devices that use INTx# and not MSI/MSI-X for their interrupts.
    // TODO: Also, balance these interrupt lines for devices that can otherwise use MSI/MSI-X.

    // Set IRQ line to 9 if not set

    let mut irq;

    unsafe {
        let mut data = cfg_access.read(bus_num, dev_num, func_num, 0x3C);
        irq = (data & 0xFF) as u8;
        if irq == 0xFF {
            irq = 9;
        }
        data = (data & 0xFFFFFF00) | irq as u32;
        cfg_access.write(bus_num, dev_num, func_num, 0x3C, data);
    };

    header.set_legacy_interrupt_line(irq);

    // Find BAR sizes
    let mut bars = [None; 6];
    let mut bar_sizes = [0u64; 6];

    {
        let wrapped_bars = match header.bars() {
            Ok(b) => b,
            Err(err) => {
                log::error!("Couldn't parse BARs for PCI device {:02x}:{:02x}.{:01x}: {}", bus_num, dev_num, func_num, err);
                return;
            }
        };

        let bar_count = wrapped_bars.len();
        bars[..bar_count].copy_from_slice(&wrapped_bars);
    }
    for (i, bar) in bars.iter().enumerate() {
        if let Some(bar) = bar {
            string.push_str(&format!(" {}={}", i, bar));
        }
    }


    unsafe {
        for (i, bar) in bars.iter().enumerate().filter_map(|(i, &bar)| Some((i, bar?))) {
            let offset = 0x10 + (i as u8) * 4;

            let is_64 = bar.is_64_bit();

            let original_lo = cfg_access.read(bus_num, dev_num, func_num, offset.into());
            cfg_access.write(bus_num, dev_num, func_num, offset.into(), 0xFFFFFFFF);

            let original_hi = if is_64 {
                let offset_hi = offset.checked_add(4).expect("expected BAR offset plus 4, not to overflow u8");
                assert!(offset_hi <= 0x28);

                let original_hi = cfg_access.read(bus_num, dev_num, func_num, offset_hi.into());
                cfg_access.write(bus_num, dev_num, func_num, offset_hi.into(), 0xFFFFFFFF);

                original_hi
            } else {
                0
            };

            let new_lo = cfg_access.read(bus_num, dev_num, func_num, offset.into());

            let new_hi = if is_64 {
                cfg_access.read(bus_num, dev_num, func_num, u16::from(offset) + 4)
            } else {
                0xFFFF_FFFF
            };

            cfg_access.write(bus_num, dev_num, func_num, offset.into(), original_lo);

            if is_64 {
                cfg_access.write(bus_num, dev_num, func_num, u16::from(offset) + 4, original_hi);
            }

            let new = (u64::from(new_hi) << 32) | u64::from(new_lo);

            let masked = if new & 1 == 1 {
                // I/O space
                assert_eq!(!new_hi, 0, "I/O space BARs can only be 32 bits");

                new & 0xFFFF_FFFC
            } else {
                // Memory space
                new & 0xFFFF_FFFF_FFFF_FFF0
            };

            let size = !masked + 1;
            bar_sizes[i] = if size <= 1 { 0 } else { size };
            log::debug!("Read BAR size for BAR {}: {}", i, size);

        }
        for i in 0..6u16 {
            log::debug!("BAR {} value: {:#08x}", i, cfg_access.read(bus_num, dev_num, func_num, u16::from(0x10 + i * 4)));
        }
    }

    let bus = PciBus {
        pci: state.preferred_cfg_access(),
        num: bus_num,
    };
    let dev = PciDev {
        bus: &bus,
        num: dev_num,
    };
    let func = PciFunc {
        dev: &dev,
        num: func_num,
    };
    let pci_capabilities = if header.base().status & (1 << 4) != 0 {
        pcicap::CapabilitiesIter(pcicap::CapabilityOffsetsIter::new(
            header.cap_pointer(),
            &func,
        ))
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let pcie_capabilities = {
        let supports_ext = cfg_access.supports_ext(bus_num);
        let has_pcie_cap = pci_capabilities.iter().any(|(_, cap)| cap.is_pcie());

        if supports_ext && has_pcie_cap {
            unsafe { pciecap::CapabilitiesIter(pciecap::CapabilityOffsetsIter::new(0x100, &func)) }
                .collect::<Vec<_>>()
        } else {
            log::debug!("Skipping PCIe capability enumeration. supports_ext={}, has_pcie_cap={}", supports_ext, has_pcie_cap);
            Vec::new()
        }
    };

    log::info!("{}. PCI capabilities: {:?}, PCI Express capabilities: {:?}", string, pci_capabilities, pcie_capabilities);

    let func = Arc::new(RwLock::new(Func {
        pci_capabilities,
        pcie_capabilities,
        header,
        bar_sizes,
    }));

    let address32 = PciAddress32::default()
        .with_seg_group(0) // TODO
        .with_bus(bus_num)
        .with_device(dev_num)
        .with_function(func_num);

    tree.functions.insert(address32, Arc::clone(&func));
}

fn setup_logging(verbosity: u8) -> Option<&'static RedoxLogger> {
    let log_level = match verbosity {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    let mut logger = RedoxLogger::new()
        .with_process_name("pcid".into())
        .with_output(
            OutputBuilder::stderr()
                .with_ansi_escape_codes()
                .with_filter(log_level)
                .flush_on_newline(true)
                .build()
         );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.log") {
        Ok(b) => {
            logger = logger.with_output(
                b.with_filter(log::LevelFilter::Debug)
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(error) => eprintln!("pcid: failed to open pcid.log"),
    }
    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.ansi.log") {
        Ok(b) => {
            logger = logger.with_output(
                b.with_filter(log::LevelFilter::Debug)
                    .with_ansi_escape_codes()
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(error) => eprintln!("pcid: failed to open pcid.ansi.log"),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("pcid: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("pcid: failed to set default logger: {}", error);
            None
        }
    }
}

fn setup_scheme(
    schemefd: usize,
    tree: Arc<RwLock<DeviceTree>>,
    state: Arc<State>,
) -> syscall::Result<()> {
    //
    // We give pcid a generous amount of resources here, since it may communicate with lots of
    // different drivers, especially when it comes to interrupts. INTx# and MSI-X interrupts tend
    // to work without any pcid involvement, since masking INTx# is usually done using device
    // registers in driver-mapped BARs, as with MSI-X. MSI however, which still is common, requires
    // the PCI configuration space to be changed, since the masking bits are stored in the
    // capability structure.
    //
    // Because this may require I/O instructions (legacy PCI 3.0) or otherwise accessing memory
    // used by other devices (PCIe MMCFG), there will be IPC between pcid and other drivers.
    // io_urings are thus a great way to keep the latency low when masking MSI interrupts.
    //

    // The following consumer instance is attached to the kernel and is used to poll the status of
    // every other io_uring when that is not done by busy-waiting, and for MSI/MSI-X IRQs used by
    // PCIe AER (TODO).
    let consumer_instance = ConsumerInstanceBuilder::new()
        .with_submission_entry_count(1024) // 64KiB
        .with_completion_entry_count(2048) // 64KiB
        .create_instance()
        .and_log_err_as_error("failed to create io_uring instance")?
        .map_all()
        .and_log_err_as_error("failed to map io_uring offsets")?
        .attach_to_kernel()
        .and_log_err_as_error("failed to attach io_uring to kernel")?;
    println!(
        "#SQ = {} #CQ = {}",
        unsafe {
            consumer_instance
                .sender()
                .as_64()
                .unwrap()
                .ring_header()
                .size
        },
        unsafe {
            consumer_instance
                .receiver()
                .as_64()
                .unwrap()
                .ring_header()
                .size
        }
    );

    let reactor = {
        let mut reactor_builder = ReactorBuilder::new();

        // safe because we're attaching it to the kernel
        reactor_builder = unsafe { reactor_builder.assume_trusted_instance() };

        reactor_builder
            .with_primary_instance(consumer_instance)
            .build()
    };
    let executor = Executor::with_reactor(reactor);
    let spawn_handle = executor.spawn_handle();
    let handle = executor
        .reactor_handle()
        .expect("expected the executor to have an integrated reactor");
    let reactor_handle = handle.clone();

    const SIMULTANEOUS_PACKET_COUNT: usize = 64;

    let main_ring = handle.reactor().primary_instance();

    let scheme_fut = async move {
        let scheme = PcidScheme::new(spawn_handle, reactor_handle, tree, state);

        log::info!("`pci:` scheme initialized, listening for requests");

        let mut packets = vec! [Packet::default(); SIMULTANEOUS_PACKET_COUNT];

        'handle_scheme: loop {
            let bytes_read = 'retry_reading: loop {
                unsafe {
                    let packet_buf = slice::from_raw_parts_mut(
                        packets.as_ptr() as *mut u8,
                        packets.len() * mem::size_of::<Packet>(),
                    );
                    match handle.read_unchecked(main_ring, SubmissionContext::new(), schemefd, packet_buf).await {
                        Ok(count) => break 'retry_reading count,
                        Err(error) if error == Error::new(EINTR) => continue 'retry_reading,
                        Err(other) => {
                            log::error!(
                                "Failed to read bytes from scheme socket, closing scheme: {}",
                                other
                            );
                            break 'handle_scheme;
                        }
                    }
                }
            };

            if bytes_read == 0 {
                log::debug!("Read zero bytes from scheme socket, thus closing scheme...");
                break 'handle_scheme;
            }

            if bytes_read % mem::size_of::<Packet>() != 0 {
                log::warn!("Read from scheme socket resulted in a number of bytes not divisible by the packet size.\n{} % {} != 0", bytes_read, mem::size_of::<Packet>());
            }
            let packets_read = bytes_read / mem::size_of::<Packet>();

            {
                let packets = &mut packets[..packets_read];

                for mut packet in packets {
                    // TODO: scheme.async_handle if required
                    log::debug!("Packet previously: {:?}", packet);
                    scheme.handle(&mut packet);
                    log::debug!("Packet after: {:?}", packet);
                }
            }

            let bytes_written = 'retry_writing: loop {
                unsafe {
                    let packet_buf = slice::from_raw_parts(
                        packets.as_ptr() as *const u8,
                        packets_read * mem::size_of::<Packet>(),
                    );
                    match handle.write_unchecked(main_ring, SubmissionContext::new(), schemefd, packet_buf).await {
                        Ok(count) => break 'retry_writing count,
                        Err(error) if error == Error::new(EINTR) => continue 'retry_writing,
                        Err(other) => {
                            log::warn!(
                                "Failed to write to scheme socket, closing scheme: {}",
                                other
                            );
                            break 'handle_scheme;
                        }
                    }
                }
            };
            if bytes_written == 0 {
                log::warn!("Wrote zero bytes to scheme socket, thus closing scheme...");
                break 'handle_scheme;
            }
        }
        log::debug!("Closing `pci:` socket");

        unsafe {
            handle.close(
                main_ring, SubmissionContext::new(), schemefd, // unused since a scheme socket ain't a disk
                true,
            )
        }
        .await
        .and_log_err_as_error("failed to close scheme socket")?;

        Ok(())
    };

    executor.run(scheme_fut)
}

fn run_scheme(
    schemefd: usize,
    tree: Arc<RwLock<DeviceTree>>,
    state: Arc<State>,
) -> syscall::Result<()> {
    match setup_scheme(schemefd, tree, state) {
        Ok(()) => Ok(()),
        Err(error) => {
            log::error!("`pci:` scheme failed to setup: \"{}\"", error);
            Err(error)
        }
    }
}

fn only_inform_about_file_scheme() {
    std::fs::write(
        "pci:read_config_dir",
        env::args()
            .nth(2)
            .expect("expected argument after --add-config-dir"),
    )
    .expect("failed to inform pci scheme that file: has appeared");
    println!("pcid inform syscall finished");
}
fn load_config_dir<P: ?Sized + AsRef<std::path::Path>>(config_path: &P, config: &mut Config) {
    let config_path = config_path.as_ref();

    let paths = match read_dir(&config_path) {
        Ok(d) => d,
        Err(err) => {
            eprintln!("pcid: failed to read configuration directory at `{}`: {}. Reverting to the default config.", config_path.as_os_str().to_string_lossy(), err);
            return;
        }
    };

    let mut config_data = String::new();

    for entry in paths {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "pcid: failed to retrieve path for directory iterator at `{}`: {}, skipping.",
                    config_path.as_os_str().to_string_lossy(),
                    err
                );
                continue;
            }
        };

        let mut config_file = match File::open(&entry.path()) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("pcid: failed to open config file `{file}` within config dir `{dir}`: {err}. Skipping config file.", file=entry.path().as_os_str().to_string_lossy(), dir=config_path.as_os_str().to_string_lossy(), err=err);
                continue;
            }
        };
        // TODO: read_to_string says it'll append to the String, so this temporary isn't required,
        // right?
        let mut tmp = String::new();

        match config_file.read_to_string(&mut tmp) {
            Ok(_bytes_read) => config_data.push_str(&tmp),
            Err(err) => {
                eprintln!("pcid: failed to read from config file `{file}` within config dir `{dir}`: {err}. Skipping config file.", file=entry.path().as_os_str().to_string_lossy(), dir=config_path.as_os_str().to_string_lossy(), err=err);
                continue;
            }
        }
    }

    match toml::from_str(&config_data) {
        Ok(cfg) => *config = cfg,
        Err(err) => {
            eprintln!("pcid: couldn't parse configuration data from files in `{}`: {}. Reverting to the default config", config_path.as_os_str().to_string_lossy(), err);
            return;
        }
    }
}
fn load_config_file<P: ?Sized + AsRef<std::path::Path>>(config_path: &P, config: &mut Config) {
    let config_path = config_path.as_ref();
    let config_data = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(error) => {
            eprintln!(
                "pcid: failed to read config from `{}`: {}, reverting to the default config",
                config_path.as_os_str().to_string_lossy(),
                error
            );
            return;
        }
    };

    match toml::from_str(&config_data) {
        Ok(cfg) => *config = cfg,
        Err(err) => {
            eprintln!(
                "pcid: invalid config data at `{}`: {}, reverting to the default config",
                config_path.as_os_str().to_string_lossy(),
                err
            );
            return;
        }
    }
}

fn main() {
    use clap::{App, Arg};

    let args = App::new("pcid")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .value_name("CONFIG")
        )
        .arg(
            Arg::with_name("verbose")
                .short("v")
                .long("verbose")
        )
        .arg(
            Arg::with_name("add-config-dir")
                .long("add-config-dir")
        )
        .get_matches();

    let mut config = Config::default();

    if args.is_present("add-config-dir") {
        // TODO: Find a better way, by letting some other process write to the pci: scheme from init.
        return only_inform_about_file_scheme();
    }

    let mut config = Config::default();

    if let Some(config_path) = args.value_of("config") {
        if metadata(&config_path).unwrap().is_file() {
            load_config_file(&config_path, &mut config);
        } else {
            load_config_dir(&config_path, &mut config);
        }
    }

    let verbosity: u8 = args.occurrences_of("verbose")
        .try_into()
        .expect("sorry, we don't want to create a black hole from all logs");
    let _logger_ref = setup_logging(verbosity);

    let pci = Arc::new(Pci::new());

    let state = Arc::new(State {
        pci: Arc::clone(&pci),
        pcie: match Pcie::new(Arc::clone(&pci)) {
            Ok(pcie) => {
                log::info!("Using PCI Express memory-mapped extended configuration space");
                Some(pcie)
            }
            Err(error) => {
                log::debug!("Couldn't retrieve PCIe info, perhaps the kernel is not compiled with acpi? Using the PCI 3.0 configuration space instead. Error: {:?}", error);
                log::info!("Using PCI 3.0 port I/O configuration space");
                None
            }
        },
        threads: Mutex::new(Vec::new()),
        bare_commands: Mutex::new(Vec::new()),
    });

    let cfg_access = state.preferred_cfg_access();

    let mut device_tree = DeviceTree {
        busses: BTreeSet::new(),
        devices: BTreeSet::new(),
        functions: BTreeMap::new(),

        // TODO
        uses_seg_groups: false,
    };

    log::info!("PCI ENV: {:?}", env::vars().collect::<Vec<_>>());

    // Open scheme socket early to prevent subdrivers from not having `pci:`.
    let schemefd = syscall::open(":pci", O_CREAT | O_EXCL | O_RDWR)
        .expect("pcid: failed to open scheme socket");

    log::info!("PCI BS/DV/FN VEND:DEVI CL.SC.IN.RV");

    // Enumerate the bus, filling the Device Tree with information about the devices. This only
    // happens once, although different drivers may get started by pcid at different stages (for
    // drivers that lay on disk, for example).
    'bus: for bus in PciIter::new(cfg_access) {
        'dev: for dev in bus.devs() {
            for func in dev.funcs() {
                let func_num = func.num;
                match PciHeader::from_reader(func) {
                    Ok(header) => {
                        let is_multifunction = header.base().header_type.multifunction;

                        // TODO: PCIe Segment Groups
                        let _ = device_tree.busses.insert((0, bus.num));
                        let _ = device_tree.devices.insert((0, bus.num, dev.num));
                        handle_parsed_header(
                            Arc::clone(&state),
                            &mut device_tree,
                            bus.num,
                            dev.num,
                            func_num,
                            header,
                        );
                        if !is_multifunction {
                            continue 'dev;
                        }
                    }
                    Err(PciHeaderError::NoDevice) => {
                        if func_num == 0 {
                            if dev.num == 0 {
                                log::trace!("PCI {:>02X}: no bus", bus.num);
                                continue 'bus;
                            } else {
                                log::trace!("PCI {:>02X}/{:>02X}: no dev", bus.num, dev.num);
                                continue 'dev;
                            }
                        }
                    }
                    Err(PciHeaderError::UnknownHeaderType(id)) => {
                        log::warn!(
                            "unknown header type for function {:02x}:{:02x}.{:01x}: {}",
                            bus.num,
                            dev.num,
                            func_num,
                            id
                        );
                    }
                    Err(PciHeaderError::InvalidBars) => {
                        log::warn!(
                            "invalid bars for function {:02x}:{:02x}.{:01x}",
                            bus.num,
                            dev.num,
                            func_num
                        );
                    }
                }
            }
        }
    }

    process_config(&config, &device_tree, &state);

    let device_tree = Arc::new(RwLock::new(device_tree));

    match run_scheme(schemefd, device_tree, Arc::clone(&state)) {
        Ok(()) => log::info!("`pci:` scheme unmounted"),
        Err(error) => log::error!("`pci:` scheme failed: \"{}\"", error),
    }

    for thread in state.threads.lock().unwrap().drain(..) {
        thread.join().unwrap();
    }
    for mut child in state.bare_commands.lock().unwrap().drain(..) {
        match child.try_wait() {
            Ok(Some(status)) => log::info!("waited for {:?}, returned status {}", child, status),
            Ok(None) => {
                // TODO: Timeouts, or the grim reaper. (Or just letting some other process manage
                // subdrivers, which seems much more optimal in the long term).
                log::debug!("child {:?} hasn't finished yet, waiting for it...", child);
                match child.wait() {
                    Ok(status) => log::info!("waited for {:?} after some delay, it finished with status {}", child, status),
                    Err(err) => log::error!("failed to wait for child {:?}: {}", child, err),
                }
            }
            Err(err) => log::error!("failed to check status and terminate child {:?}: {}", child, err),
        }
    }
    log::info!("exited pcid");
}
