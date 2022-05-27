#![feature(array_chunks, array_zip)]

use std::collections::BTreeMap;
use std::sync::Arc;

use syscall::data::Packet;
use syscall::error::EINTR;
use syscall::flag::{O_RDWR, O_CREAT, O_CLOEXEC};
use syscall::scheme::SchemeMut;

use log::{info, warn, trace};
use redox_log::{OutputBuilder, RedoxLogger};

pub use pcid_lib::{pci, pcie};

use crate::pci::{CfgAccess, Pci, PciIter, PciBar, PciBus, PciClass, PciDev, PciFunc, PciHeader, PciHeaderError, PciHeaderType};
use crate::pci::cap::Capability as PciCapability;
use crate::pcie::Pcie;
pub use pcid_lib::{driver_interface, PciAddr};

mod scheme;

fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(pci: &dyn CfgAccess, addr: PciAddr, function: F) -> T {
    let bus = PciBus {
        pci,
        num: addr.bus,
    };
    let dev = PciDev {
        bus: &bus,
        num: addr.dev,
    };
    let func = PciFunc {
        dev: &dev,
        num: addr.func,
    };
    function(&func)
}
fn read_bar_sizes(pci: &dyn CfgAccess, addr: PciAddr, header: &PciHeader) -> [(PciBar, u32); 6] {
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

            let original = pci.read(addr, offset.into());
            pci.write(addr, offset.into(), 0xFFFFFFFF);

            let new = pci.read(addr, offset.into());
            pci.write(addr, offset.into(), original);

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

    bars.zip(bar_sizes)
}
fn handle_channel_request(state: &State, tree: &mut BTreeMap<PciAddr, Func>, addr: PciAddr, request: driver_interface::PcidClientRequest) -> driver_interface::PcidClientResponse {
    use driver_interface::*;
    use crate::pci::cap::{MsiCapability, MsixCapability};

    let func = match tree.get_mut(&addr) {
        Some(f) => f,
        None => return PcidClientResponse::Error(PcidServerResponseError::InternalError("function not found".into())),
    };
    let capabilities = &mut func.capabilities;

    match request {
        PcidClientRequest::RequestConfig => {
            PcidClientResponse::Config(SubdriverArguments {
                func: PciFunction {
                    bus_num: addr.bus,
                    dev_num: addr.dev,
                    func_num: addr.func,
                    venid: func.header.vendor_id(),
                    devid: func.header.device_id(),
                    bars: func.bars.map(|(b, _)| b),
                    bar_sizes: func.bars.map(|(_, s)| s),
                    legacy_interrupt_line: func.header.interrupt_line(),
                    legacy_interrupt_pin: func.header.interrupt_pin(),
                },
            })
        }
        PcidClientRequest::RequestFeatures => {
            PcidClientResponse::AllFeatures(capabilities.iter().filter_map(|(_, capability)| match capability {
                PciCapability::Msi(msi) => Some((PciFeature::Msi, FeatureStatus::enabled(msi.enabled()))),
                PciCapability::MsiX(msix) => Some((PciFeature::MsiX, FeatureStatus::enabled(msix.msix_enabled()))),
                _ => None,
            }).collect())
        }
        PcidClientRequest::EnableFeature(feature) => match feature {
            PciFeature::Msi => {
                let (offset, capability): (u8, &mut MsiCapability) = match capabilities.iter_mut().find_map(|&mut (offset, ref mut capability)| capability.as_msi_mut().map(|cap| (offset, cap))) {
                    Some(tuple) => tuple,
                    None => return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature)),
                };
                unsafe {
                    with_pci_func_raw(state.preferred_cfg_access(), addr, |func| {
                        capability.set_enabled(true);
                        capability.write_message_control(func, offset);
                    });
                }
                PcidClientResponse::FeatureEnabled(feature)
            }
            PciFeature::MsiX => {
                let (offset, capability): (u8, &mut MsixCapability) = match capabilities.iter_mut().find_map(|&mut (offset, ref mut capability)| capability.as_msix_mut().map(|cap| (offset, cap))) {
                    Some(tuple) => tuple,
                    None => return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature)),
                };
                unsafe {
                    with_pci_func_raw(state.preferred_cfg_access(), addr, |func| {
                        capability.set_msix_enabled(true);
                        capability.write_a(func, offset);
                    });
                }
                PcidClientResponse::FeatureEnabled(feature)
            }
        }
        PcidClientRequest::FeatureStatus(feature) => PcidClientResponse::FeatureStatus(feature, match feature {
            PciFeature::Msi => capabilities.iter().find_map(|(_, capability)| if let PciCapability::Msi(msi) = capability {
                Some(FeatureStatus::enabled(msi.enabled()))
            } else {
                None
            }).unwrap_or(FeatureStatus::Disabled),
            PciFeature::MsiX => capabilities.iter().find_map(|(_, capability)| if let PciCapability::MsiX(msix) = capability {
                Some(FeatureStatus::enabled(msix.msix_enabled()))
            } else {
                None
            }).unwrap_or(FeatureStatus::Disabled),
        }),
        PcidClientRequest::FeatureInfo(feature) => PcidClientResponse::FeatureInfo(feature, match feature {
            PciFeature::Msi => if let Some(info) = capabilities.iter().find_map(|(_, capability)| capability.as_msi()) {
                PciFeatureInfo::Msi(*info)
            } else {
                return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature));
            }
            PciFeature::MsiX => if let Some(info) = capabilities.iter().find_map(|(_, capability)| capability.as_msix()) {
                PciFeatureInfo::MsiX(*info)
            } else {
                return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature));
            }
        }),
        PcidClientRequest::SetFeatureInfo(info_to_set) => match info_to_set {
            SetFeatureInfo::Msi(info_to_set) => if let Some((offset, info)) = capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msi_mut()?))) {
                if let Some(mme) = info_to_set.multi_message_enable {
                    if info.multi_message_capable() < mme || mme > 0b101 {
                        return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                    }
                    info.set_multi_message_enable(mme);

                }
                if let Some(message_addr) = info_to_set.message_address {
                    if message_addr & 0b11 != 0 {
                        return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                    }
                    info.set_message_address(message_addr);
                }
                if let Some(message_addr_upper) = info_to_set.message_upper_address {
                    info.set_message_upper_address(message_addr_upper);
                }
                if let Some(message_data) = info_to_set.message_data {
                    if message_data & ((1 << info.multi_message_enable()) - 1) != 0 {
                        return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                    }
                    info.set_message_data(message_data);
                }
                if let Some(mask_bits) = info_to_set.mask_bits {
                    info.set_mask_bits(mask_bits);
                }
                unsafe {
                    with_pci_func_raw(state.preferred_cfg_access(), addr, |func| {
                        info.write_all(func, offset);
                    });
                }
                PcidClientResponse::SetFeatureInfo(PciFeature::Msi)
            } else {
                return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(PciFeature::Msi));
            }
            SetFeatureInfo::MsiX { function_mask } => if let Some((offset, info)) = capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msix_mut()?))) {
                if let Some(mask) = function_mask {
                    info.set_function_mask(mask);
                    unsafe {
                        with_pci_func_raw(state.preferred_cfg_access(), addr, |func| {
                            info.write_a(func, offset);
                        });
                    }
                }
                PcidClientResponse::SetFeatureInfo(PciFeature::MsiX)
            } else {
                return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(PciFeature::MsiX));
            }
            _ => return PcidClientResponse::Error(PcidServerResponseError::InternalError("unknown SetFeatureInfo feature".into())),
        }
        _ => return PcidClientResponse::Error(PcidServerResponseError::InternalError("unknown client request".into())),
    }
}
pub struct State {
    pci: Arc<Pci>,
    pcie: Option<Pcie>,
}
pub struct Func {
    capabilities: Vec<(u8, PciCapability)>,
    header: PciHeader,
    bars: [(PciBar, u32); 6],
}
impl State {
    fn preferred_cfg_access(&self) -> &dyn CfgAccess {
        // TODO
        //self.pcie.as_ref().map(|pcie| pcie as &dyn CfgAccess).unwrap_or(&*self.pci as &dyn CfgAccess)
        &*self.pci as &dyn CfgAccess
    }
}

fn enable_func(pci: &dyn CfgAccess, addr: PciAddr, func: &Func) {
    // Enable bus mastering, memory space, and I/O space
    unsafe {
        let mut data = pci.read(addr, 0x04);
        data |= 7;
        pci.write(addr, 0x04, data);
    }

    // Set IRQ line to 9 if not set
    let mut irq;
    let mut interrupt_pin;

    unsafe {
        let mut data = pci.read(addr, 0x3C);
        irq = (data & 0xFF) as u8;
        interrupt_pin = ((data & 0x0000_FF00) >> 8) as u8;
        if irq == 0xFF {
            irq = 9;
        }
        data = (data & 0xFFFFFF00) | irq as u32;
        pci.write(addr, 0x3C, data);
    };

}

fn handle_parsed_header(state: &State, tree: &mut BTreeMap<PciAddr, Func>, addr: PciAddr, header: PciHeader) {
    let pci = state.preferred_cfg_access();

    let capabilities = if header.status() & (1 << 4) != 0 {
        with_pci_func_raw(state.preferred_cfg_access(), addr, |func| crate::pci::cap::CapabilitiesIter { inner: crate::pci::cap::CapabilityOffsetsIter::new(header.cap_pointer(), func) }.collect::<Vec<_>>())
    } else {
        Vec::new()
    };
    //info!("PCI DEVICE CAPABILITIES for {}: {:?}", args.iter().map(|string| string.as_ref()).nth(0).unwrap_or("[unknown]"), capabilities);

    let bars = read_bar_sizes(pci, addr, &header);

    let func = Func {
        capabilities,
        header,
        bars,
    };

    tree.insert(addr, func);
}

fn setup_logging(verbosity: u8) -> Option<&'static RedoxLogger> {
    let log_level = match verbosity {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_ansi_escape_codes()
                .with_filter(log_level)
                .flush_on_newline(true)
                .build()
         );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Trace)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("pcid: failed to open pcid.log"),
    }
    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Trace)
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

fn main() {
    let mut args = pico_args::Arguments::from_env();
    let verbosity = (0..).find(|_| !args.contains("-v")).unwrap_or(0);

    let _logger_ref = setup_logging(verbosity);

    let pci = Arc::new(Pci::new());

    let mut state = State {
        pci: Arc::clone(&pci),
        pcie: match Pcie::new(Arc::clone(&pci)) {
            Ok(pcie) => Some(pcie),
            Err(error) => {
                info!("Couldn't retrieve PCIe info, perhaps the kernel is not compiled with acpi? Using the PCI 3.0 configuration space instead. Error: {:?}", error);
                None
            }
        },
    };
    let mut tree = BTreeMap::new();

    let pci = state.preferred_cfg_access();

    info!("PCI BS/DV/FN VEND:DEVI CL.SC.IN.RV");

    'bus: for bus in PciIter::new(pci) {
        'dev: for dev in bus.devs() {
            for func in dev.funcs() {
                let func_num = func.num;

                let addr = PciAddr {
                    // TODO
                    seg: 0,
                    bus: bus.num,
                    dev: dev.num,
                    func: func.num,
                };

                match PciHeader::from_reader(func) {
                    Ok(header) => {
                        handle_parsed_header(&state, &mut tree, addr, header);
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
    info!("Enumeration complete, now starting `pci:` scheme");

    syscall::daemon::Daemon::new(move |daemon: syscall::daemon::Daemon| -> std::convert::Infallible {
        let mut scheme = self::scheme::PciScheme::new(state, tree);
        let scheme_socket = syscall::open(":pci", O_RDWR | O_CREAT | O_CLOEXEC).expect("failed to open pci scheme socket");
        let mut packet = Packet::default();

        let _ = daemon.ready();

        'main_loop: loop {
            'eintr_read_loop: loop {
                match syscall::read(scheme_socket, &mut packet) {
                    Ok(0) => break 'main_loop,
                    Ok(_) => break 'eintr_read_loop,
                    Err(error) if error.errno == EINTR => continue 'eintr_read_loop,
                    Err(error) => panic!("failed to read from scheme socket: {}", error),
                }
            }
            scheme.handle(&mut packet);

            'eintr_write_loop: loop {
                match syscall::write(scheme_socket, &packet) {
                    Ok(0) => break 'main_loop,
                    Ok(_) => break 'eintr_write_loop,
                    Err(error) if error.errno == EINTR => continue 'eintr_write_loop,
                    Err(error) => panic!("failed to write to scheme socket: {}", error),
                }
            }
        }
        let _ = syscall::exit(0);
        unreachable!();
    }).expect("failed to fork pcid");
}
