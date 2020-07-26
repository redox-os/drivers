#![feature(llvm_asm)]

use std::fs::{File, metadata, read_dir};
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{env, mem, slice, thread};

use syscall::{
    O_CREAT, O_EXCL, O_RDWR,

    EINTR, ENOMSG,

    io_uring::{
        v1,
        IoUringCqeFlags, IoUringSqeFlags, IoUringEnterFlags,

        SqEntry64,
        CqEntry64,
    },
    CloneFlags, Error, EventFlags, Packet,
};
use syscall::scheme::Scheme as _;

use log::{error, info, warn, trace};
use redox_iou::executor::Executor;
use redox_iou::instance::ConsumerInstanceBuilder;
use redox_iou::reactor::ReactorBuilder;
use redox_log::{OutputBuilder, RedoxLogger};

mod config;

#[allow(dead_code)]
mod driver_interface;

mod pci;
mod pcie;
mod scheme;

use crate::config::Config;
use crate::pci::{CfgAccess, Pci, PciIter, PciBar, PciBus, PciClass, PciDev, PciFunc, PciHeader, PciHeaderError, PciHeaderType};
use crate::pci::cap::{self as pcicap, Capability as PciCapability};
use crate::pcie::Pcie;
use crate::pcie::cap::{self as pciecap, Capability as PcieCapability};
use crate::scheme::PcidScheme;

// TODO: Move this helper trait to redox-log.
trait ResultExt where Self: Sized {
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

pub struct DriverHandler {
    config: config::DriverConfig,
    bus_num: u8,
    dev_num: u8,
    func_num: u8,
    header: PciHeader,
    pci_capabilities: Vec<(u8, PciCapability)>,
    pcie_capabilities: Vec<(u16, PcieCapability)>,

    state: Arc<State>,
}
fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(pci: &dyn CfgAccess, bus_num: u8, dev_num: u8, func_num: u8, function: F) -> T {
    let bus = PciBus {
        pci,
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
    function(&func)
}
impl DriverHandler {
    fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(&self, function: F) -> T {
        with_pci_func_raw(self.state.preferred_cfg_access(), self.bus_num, self.dev_num, self.func_num, function)
    }

    fn respond(&mut self, request: driver_interface::PcidClientRequest, args: &driver_interface::SubdriverArguments) -> driver_interface::PcidClientResponse {
        use driver_interface::*;

        match request {
            PcidClientRequest::RequestConfig => {
                PcidClientResponse::Config(args.clone())
            }
            PcidClientRequest::GetCapabilities => {
                PcidClientResponse::AllCapabilities(
                    self.pci_capabilities.iter().map(|(_, capability)| Capability::Pci(capability.clone()))
                        .chain(self.pcie_capabilities.iter().map(|(_, capability)| Capability::Pcie(capability.clone())))
                        .collect()
                )
            }
            PcidClientRequest::GetCapability(ty) => PcidClientResponse::Capability(match ty {
                CapabilityType::Msi => self.pci_capabilities.iter().find_map(|(_, capability)| capability.as_msi().copied()).map(PciCapability::Msi).map(Capability::Pci),
                CapabilityType::MsiX => self.pci_capabilities.iter().find_map(|(_, capability)| capability.as_msix().copied()).map(PciCapability::MsiX).map(Capability::Pci),
                // TODO
                other => return PcidClientResponse::Error(PcidServerResponseError::NonexistentCapability(other)),
            }),
            PcidClientRequest::SetCapability(info_to_set) => {
                let mut msi_enabled = false;
                let mut msix_enabled = false;

                for cap in self.pci_capabilities.iter() {
                    match cap {
                        &(_, PciCapability::Msi(ref cap)) => msi_enabled = cap.enabled(),
                        &(_, PciCapability::MsiX(ref cap)) => msix_enabled = cap.msix_enabled(),
                        _ => (),
                    }
                }

                match info_to_set {
                    SetCapabilityInfo::Msi(info_to_set) => if let Some((offset, info)) = self.pci_capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msi_mut()?))) {
                        let info_to_set_flags = match MsiSetCapabilityInfoFlags::from_bits(info_to_set.flags) {
                            Some(f) => f,
                            None => return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern),
                        };

                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::ENABLED) && info_to_set.enabled == true as u8 {
                            if msix_enabled {
                                log::error!("Client trying to enable MSI while MSI-X is already enabled.");
                                return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                            }
                        }

                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MULTI_MESSAGE_ENABLE) {
                            let mme = info_to_set.multi_message_enable;
                            if info.multi_message_capable() < mme || mme > 0b101 {
                                return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                            }
                            info.set_multi_message_enable(mme);

                        }
                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MESSAGE_ADDRESS) {
                            let message_addr = info_to_set.message_address;
                            if message_addr & 0b11 != 0 {
                                return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                            }
                            info.set_message_address(message_addr);
                        }
                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MESSAGE_UPPER_ADDRESS) {
                            let message_addr_upper = info_to_set.message_upper_address;
                            info.set_message_upper_address(message_addr_upper);
                        }
                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MESSAGE_DATA) {
                            let message_data = info_to_set.message_data;
                            if message_data & ((1 << info.multi_message_enable()) - 1) != 0 {
                                return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                            }
                            info.set_message_data(message_data);
                        }
                        if info_to_set_flags.contains(MsiSetCapabilityInfoFlags::MASK_BITS) {
                            let mask_bits = info_to_set.mask_bits;
                            info.set_mask_bits(mask_bits);
                        }
                        unsafe {
                            with_pci_func_raw(self.state.preferred_cfg_access(), self.bus_num, self.dev_num, self.func_num, |func| {
                                info.write_all(func, offset);
                            });
                        }
                        PcidClientResponse::SetCapability
                    } else {
                        return PcidClientResponse::Error(PcidServerResponseError::NonexistentCapability(CapabilityType::Msi));
                    }
                    SetCapabilityInfo::MsiX(MsiXSetCapabilityInfo { function_mask, enabled, flags }) => if let Some((offset, info)) = self.pci_capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msix_mut()?))) {
                        let mut write = false;

                        let flags = match MsiXSetCapabilityInfoFlags::from_bits(flags) {
                            Some(f) => f,
                            None => return driver_interface::PcidClientResponse::Error(driver_interface::PcidServerResponseError::InvalidBitPattern),
                        };
                        if flags.contains(MsiXSetCapabilityInfoFlags::ENABLED) {
                            if msi_enabled {
                                log::error!("Client trying to enable MSI-X while MSI is already enabled.");
                                return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
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
                                with_pci_func_raw(self.state.preferred_cfg_access(), self.bus_num, self.dev_num, self.func_num, |func| {
                                    info.write_a(func, offset);
                                });
                            }
                        }
                        PcidClientResponse::SetCapability
                    } else {
                        return PcidClientResponse::Error(PcidServerResponseError::NonexistentCapability(CapabilityType::MsiX));
                    }
                }
            }
        }
    }
    fn handle_spawn(mut self, pcid_to_client_write: Option<usize>, pcid_from_client_read: Option<usize>, args: driver_interface::SubdriverArguments) {
        use driver_interface::*;

        if let (Some(pcid_to_client_fd), Some(pcid_from_client_fd)) = (pcid_to_client_write, pcid_from_client_read) {
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
    pci: Arc<Pci>,
    pcie: Option<Pcie>,
}
impl State {
    fn preferred_cfg_access(&self) -> &dyn CfgAccess {
        self.pcie.as_ref().map(|pcie| pcie as &dyn CfgAccess).unwrap_or(&*self.pci as &dyn CfgAccess)
    }
}

fn handle_parsed_header(state: Arc<State>, config: &Config, bus_num: u8,
                        dev_num: u8, func_num: u8, header: PciHeader) {
    let pci = state.preferred_cfg_access();

    let raw_class: u8 = header.class().into();
    let mut string = format!("PCI {:>02X}/{:>02X}/{:>02X} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
                             bus_num, dev_num, func_num, header.vendor_id(), header.device_id(), raw_class,
                             header.subclass(), header.interface(), header.revision(), header.class());
    match header.class() {
        PciClass::Legacy if header.subclass() == 1 => string.push_str("  VGA CTL"),
        PciClass::Storage => match header.subclass() {
            0x01 => {
                string.push_str(" IDE");
            },
            0x06 => if header.interface() == 0 {
                string.push_str(" SATA VND");
            } else if header.interface() == 1 {
                string.push_str(" SATA AHCI");
            },
            _ => ()
        },
        PciClass::SerialBus => match header.subclass() {
            0x03 => match header.interface() {
                0x00 => {
                    string.push_str(" UHCI");
                },
                0x10 => {
                    string.push_str(" OHCI");
                },
                0x20 => {
                    string.push_str(" EHCI");
                },
                0x30 => {
                    string.push_str(" XHCI");
                },
                _ => ()
            },
            _ => ()
        },
        _ => ()
    }

    for (i, bar) in header.bars().iter().enumerate() {
        if !bar.is_none() {
            string.push_str(&format!(" {}={}", i, bar));
        }
    }

    info!("{}", string);

    for driver in config.drivers.iter() {
        if let Some(class) = driver.class {
            if class != raw_class { continue; }
        }

        if let Some(subclass) = driver.subclass {
            if subclass != header.subclass() { continue; }
        }

        if let Some(interface) = driver.interface {
            if interface != header.interface() { continue; }
        }

        if let Some(ref ids) = driver.ids {
            let mut device_found = false;
            for (vendor, devices) in ids {
                let vendor_without_prefix = vendor.trim_start_matches("0x");
                let vendor = i64::from_str_radix(vendor_without_prefix, 16).unwrap() as u16;

                if vendor != header.vendor_id() { continue; }

                for device in devices {
                    if *device == header.device_id() {
                        device_found = true;
                        break;
                    }
                }
            }
            if !device_found { continue; }
        } else {
            if let Some(vendor) = driver.vendor {
                if vendor != header.vendor_id() { continue; }
            }

            if let Some(device) = driver.device {
                if device != header.device_id() { continue; }
            }
        }

        if let Some(ref device_id_range) = driver.device_id_range {
            if header.device_id() < device_id_range.start  ||
               device_id_range.end <= header.device_id() { continue; }
        }

        if let Some(ref args) = driver.command {
            // Enable bus mastering, memory space, and I/O space
            unsafe {
                let mut data = pci.read(bus_num, dev_num, func_num, 0x04);
                data |= 7;
                pci.write(bus_num, dev_num, func_num, 0x04, data);
            }

            // Set IRQ line to 9 if not set
            let mut irq;
            let interrupt_pin;

            unsafe {
                let mut data = pci.read(bus_num, dev_num, func_num, 0x3C);
                irq = (data & 0xFF) as u8;
                interrupt_pin = ((data & 0x0000_FF00) >> 8) as u8;
                if irq == 0xFF {
                    irq = 9;
                }
                data = (data & 0xFFFFFF00) | irq as u32;
                pci.write(bus_num, dev_num, func_num, 0x3C, data);
            };

            // Find BAR sizes
            let mut bars = [PciBar::None; 6];
            let mut bar_sizes = [0; 6];
            unsafe {
                let count = match header.header_type() {
                    PciHeaderType::GENERAL => 6,
                    PciHeaderType::PCITOPCI => 2,
                    _ => 0,
                };

                for i in 0..count {
                    bars[i] = header.get_bar(i);

                    let offset = 0x10 + (i as u8) * 4;

                    let original = pci.read(bus_num, dev_num, func_num, offset.into());
                    pci.write(bus_num, dev_num, func_num, offset.into(), 0xFFFFFFFF);

                    let new = pci.read(bus_num, dev_num, func_num, offset.into());
                    pci.write(bus_num, dev_num, func_num, offset.into(), original);

                    let masked = if new & 1 == 1 {
                        new & 0xFFFFFFFC
                    } else {
                        new & 0xFFFFFFF0
                    };

                    let size = !masked + 1;
                    bar_sizes[i] = if size <= 1 {
                        0
                    } else {
                        size
                    };
                }
            }

            let bus = PciBus {
                pci: state.preferred_cfg_access(),
                num: bus_num,
            };
            let dev = PciDev {
                bus: &bus,
                num: dev_num
            };
            let func = PciFunc {
                dev: &dev,
                num: func_num,
            };
            let pci_capabilities = if header.status() & (1 << 4) != 0 {
                pcicap::CapabilitiesIter(pcicap::CapabilityOffsetsIter::new(header.cap_pointer(), &func)).collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let driver_name = args.iter().map(|string| string.as_ref()).nth(0).unwrap_or("[unknown]");
            info!("PCI DEVICE CAPABILITIES for {}: {:?}", driver_name, pci_capabilities);

            let pcie_capabilities = if pci.supports_ext(bus_num) && pci_capabilities.iter().any(|(_, cap)| cap.is_pcie()) {
                unsafe { pciecap::CapabilitiesIter(pciecap::CapabilityOffsetsIter::new(0x100, &func)) }.collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            info!("PCIe DEVICE CAPABILITIES FOR {}: {:?}", driver_name, pcie_capabilities);

            use driver_interface::LegacyInterruptPin;

            let legacy_interrupt_pin = match interrupt_pin {
                0 => None,
                1 => Some(LegacyInterruptPin::IntA),
                2 => Some(LegacyInterruptPin::IntB),
                3 => Some(LegacyInterruptPin::IntC),
                4 => Some(LegacyInterruptPin::IntD),

                other => {
                    warn!("pcid: invalid interrupt pin: {}", other);
                    None
                }
            };

            let func = driver_interface::PciFunction {
                bars,
                bar_sizes,
                bus_num,
                dev_num,
                func_num,
                devid: header.device_id(),
                legacy_interrupt_line: irq,
                legacy_interrupt_pin: legacy_interrupt_pin.map_or(0, |pin| pin as u8),
                venid: header.vendor_id(),
            };

            let subdriver_args = driver_interface::SubdriverArguments {
                func,
            };

            let mut args = args.iter();
            if let Some(program) = args.next() {
                let mut command = Command::new(program);
                for arg in args {
                    let arg = match arg.as_str() {
                        "$BUS" => format!("{:>02X}", bus_num),
                        "$DEV" => format!("{:>02X}", dev_num),
                        "$FUNC" => format!("{:>02X}", func_num),
                        "$NAME" => func.name(),
                        "$BAR0" => format!("{}", bars[0]),
                        "$BAR1" => format!("{}", bars[1]),
                        "$BAR2" => format!("{}", bars[2]),
                        "$BAR3" => format!("{}", bars[3]),
                        "$BAR4" => format!("{}", bars[4]),
                        "$BAR5" => format!("{}", bars[5]),
                        "$BARSIZE0" => format!("{:>08X}", bar_sizes[0]),
                        "$BARSIZE1" => format!("{:>08X}", bar_sizes[1]),
                        "$BARSIZE2" => format!("{:>08X}", bar_sizes[2]),
                        "$BARSIZE3" => format!("{:>08X}", bar_sizes[3]),
                        "$BARSIZE4" => format!("{:>08X}", bar_sizes[4]),
                        "$BARSIZE5" => format!("{:>08X}", bar_sizes[5]),
                        "$IRQ" => format!("{}", irq),
                        "$VENID" => format!("{:>04X}", header.vendor_id()),
                        "$DEVID" => format!("{:>04X}", header.device_id()),
                        _ => arg.clone()
                    };
                    command.arg(&arg);
                }

                info!("PCID SPAWN {:?}", command);

                let (pcid_to_client_write, pcid_from_client_read, envs) = if driver.use_channel.unwrap_or(false) {
                    let mut fds1 = [0usize; 2];
                    let mut fds2 = [0usize; 2];

                    syscall::pipe2(&mut fds1, 0).expect("pcid: failed to create pcid->client pipe");
                    syscall::pipe2(&mut fds2, 0).expect("pcid: failed to create client->pcid pipe");

                    let [pcid_to_client_read, pcid_to_client_write] = fds1;
                    let [pcid_from_client_read, pcid_from_client_write] = fds2;

                    (Some(pcid_to_client_write), Some(pcid_from_client_read), vec! [("PCID_TO_CLIENT_FD", format!("{}", pcid_to_client_read)), ("PCID_FROM_CLIENT_FD", format!("{}", pcid_from_client_write))])
                } else {
                    (None, None, vec! [])
                };

                match command.envs(envs).spawn() {
                    Ok(mut child) => {
                        let driver_handler = DriverHandler {
                            bus_num,
                            dev_num,
                            func_num,
                            config: driver.clone(),
                            header,
                            state: Arc::clone(&state),
                            pci_capabilities,
                            pcie_capabilities,
                        };
                        let thread = thread::spawn(move || {
                            driver_handler.handle_spawn(pcid_to_client_write, pcid_from_client_read, subdriver_args);
                        });
                        match child.wait() {
                            Ok(_status) => (),
                            Err(err) => error!("pcid: failed to wait for {:?}: {}", command, err),
                        }
                    }
                    Err(err) => error!("pcid: failed to execute {:?}: {}", command, err)
                }
            }
        }
    }
}

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_process_name("pcid".into())
        /*.with_output(
            OutputBuilder::stderr()
                .with_ansi_escape_codes()
                .with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
         )*/
        .with_output(
            OutputBuilder::with_endpoint(std::fs::OpenOptions::new()
                .create_new(false)
                .read(false)
                .write(true)
                .open("debug:").unwrap()
            )
                .with_ansi_escape_codes()
                //.with_filter(log::LevelFilter::Trace)
                .with_filter(log::LevelFilter::Debug)
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Debug)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("pcid: failed to open pcid.log"),
    }
    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Debug)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
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

fn setup_scheme(schemefd: usize) -> syscall::Result<()> {
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
        .with_submission_entry_count(1024)  // 64KiB
        .with_completion_entry_count(2048)  // 64KiB
        .create_instance().and_log_err_as_error("failed to create io_uring instance")?
        .map_all().and_log_err_as_error("failed to map io_uring offsets")?
        .attach_to_kernel().and_log_err_as_error("failed to attach io_uring to kernel")?;
    println!("#SQ = {} #CQ = {}", unsafe { consumer_instance.sender().as_64().unwrap().ring_header().size }, unsafe { consumer_instance.receiver().as_64().unwrap().ring_header().size });

    let reactor = {
        let mut reactor_builder = ReactorBuilder::new();

        // safe because we're attaching it to the kernel
        reactor_builder = unsafe { reactor_builder.assume_trusted_instance() };

        reactor_builder.with_primary_instance(consumer_instance).build()
    };
    let executor = Executor::with_reactor(reactor);
    let spawn_handle = executor.spawn_handle();
    let handle = executor.reactor_handle().expect("expected the executor to have an integrated reactor");
    let reactor_handle = handle.clone();

    const SIMULTANEOUS_PACKET_COUNT: usize = 64;

    let main_ring = handle.reactor().primary_instance();

    let scheme_fut = async move {
        let scheme = PcidScheme::new(spawn_handle, reactor_handle);

        log::info!("`pci:` scheme initialized, listening for requests");

        let mut packets = [Packet::default(); SIMULTANEOUS_PACKET_COUNT];

        'handle_scheme: loop {
            let bytes_read = 'retry_reading: loop {
                unsafe {
                    let packet_buf = slice::from_raw_parts_mut(packets.as_ptr() as *mut u8, packets.len() * mem::size_of::<Packet>());
                    match handle.read(main_ring, schemefd, packet_buf).await {
                        Ok(count) => break 'retry_reading count,
                        Err(error) if error == Error::new(EINTR) => continue 'retry_reading,
                        Err(other) => {
                            log::error!("Failed to read bytes from scheme socket, closing scheme: {}", other);
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
                    // TODO: The assumption that the `pci:` scheme does nothing but listing an empty
                    // dir, may not hold if it's also going to be used for IPC as well.
                    scheme.handle(&mut packet);
                }
            }

            let bytes_written = 'retry_writing: loop {
                unsafe {
                    let packet_buf = slice::from_raw_parts(packets.as_ptr() as *const u8, packets_read * mem::size_of::<Packet>());
                    match handle.write(main_ring, schemefd, packet_buf).await {
                        Ok(count) => break 'retry_writing count,
                        Err(error) if error == Error::new(EINTR) => continue 'retry_writing,
                        Err(other) => {
                            log::warn!("Failed to write to scheme socket, closing scheme: {}", other);
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
                main_ring,
                schemefd,

                // unused since a scheme socket ain't a disk
                true
            )
        }.await.and_log_err_as_error("failed to close scheme socket")?;

        Ok(())
    };

    executor.run(scheme_fut)
}

fn run_scheme(schemefd: usize) -> syscall::Result<()> {
    match setup_scheme(schemefd) {
        Ok(()) => Ok(()),
        Err(error) => {
            error!("`pci:` failed to setup: \"{}\"", error);
            Err(error)
        }
    }
}

fn main() {
    let mut config = Config::default();

    let mut args = env::args().skip(1);
    if let Some(config_path) = args.next() {
        if metadata(&config_path).unwrap().is_file() {
            if let Ok(mut config_file) = File::open(&config_path) {
                let mut config_data = String::new();
                if let Ok(_) = config_file.read_to_string(&mut config_data) {
                    config = toml::from_str(&config_data).unwrap_or(Config::default());
                }
            }
        } else {
            let paths = read_dir(&config_path).unwrap();

            let mut config_data = String::new();

            for path in paths {
                if let Ok(mut config_file) = File::open(&path.unwrap().path()) {
                    let mut tmp = String::new();
                    if let Ok(_) = config_file.read_to_string(&mut tmp) {
                        config_data.push_str(&tmp);
                    }
                }
            }

            config = toml::from_str(&config_data).unwrap_or(Config::default());
        }
    }

    let _logger_ref = setup_logging();

    let pci = Arc::new(Pci::new());

    let state = Arc::new(State {
        pci: Arc::clone(&pci),
        pcie: match Pcie::new(Arc::clone(&pci)) {
            Ok(pcie) => Some(pcie),
            Err(error) => {
                info!("Couldn't retrieve PCIe info, perhaps the kernel is not compiled with acpi? Using the PCI 3.0 configuration space instead. Error: {:?}", error);
                None
            }
        },
        threads: Mutex::new(Vec::new()),
    });

    let pci = state.preferred_cfg_access();

    info!("PCI BS/DV/FN VEND:DEVI CL.SC.IN.RV");

    'bus: for bus in PciIter::new(pci) {
        'dev: for dev in bus.devs() {
            for func in dev.funcs() {
                let func_num = func.num;
                match PciHeader::from_reader(func) {
                    Ok(header) => {
                        handle_parsed_header(Arc::clone(&state), &config, bus.num, dev.num, func_num, header);
                    }
                    Err(PciHeaderError::NoDevice) => {
                        if func_num == 0 {
                            if dev.num == 0 {
                                trace!("PCI {:>02X}: no bus", bus.num);
                                continue 'bus;
                            } else {
                                trace!("PCI {:>02X}/{:>02X}: no dev", bus.num, dev.num);
                                continue 'dev;
                            }
                        }
                    },
                    Err(PciHeaderError::UnknownHeaderType(id)) => {
                        warn!("pcid: unknown header type: {}", id);
                    }
                }
            }
        }
    }

    if unsafe { syscall::clone(CloneFlags::empty()) }.expect("pcid: failed to fork") == 0 {
        let schemefd = syscall::open(":pci", O_CREAT | O_EXCL | O_RDWR).expect("pcid: failed to open scheme socket");

        match run_scheme(schemefd) {
            Ok(()) => info!("`pci:` scheme unmounted"),
            Err(error) => error!("`pci:` scheme failed: \"{}\"", error),
        }
        return;
    }

    info!("pcid forked, about to exit parent");

    for thread in state.threads.lock().unwrap().drain(..) {
        thread.join().unwrap();
    }
    info!("exited pcid");
}
