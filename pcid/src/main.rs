// Already stabilized, TODO: remove when Redox's rustc is updated
#![feature(result_option_inspect)]

use std::fs::{File, metadata, read_dir};
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::Command;
use std::thread;
use std::sync::{Arc, Mutex};

use pci_types::device_type::DeviceType;
use pci_types::{CommandRegister, ConfigRegionAccess, PciAddress};
use structopt::StructOpt;
use log::{debug, error, info, warn, trace};
use redox_log::{OutputBuilder, RedoxLogger};

use crate::cfg_access::Pcie;
use crate::config::Config;
use crate::driver_interface::LegacyInterruptLine;
use crate::pci::PciFunc;
use crate::pci::cap::Capability as PciCapability;
use crate::pci::func::{ConfigReader, ConfigWriter};
use crate::pci_header::{PciEndpointHeader, PciHeader, PciHeaderError};

mod cfg_access;
mod config;
mod driver_interface;
mod pci;
mod pci_header;

#[derive(StructOpt)]
#[structopt(about)]
struct Args {
    #[structopt(short, long,
        help="Increase logging level once for each arg.", parse(from_occurrences))]
    verbose: u8,

    #[structopt(
        help="A path to a pcid config file or a directory that contains pcid config files.")]
    config_path: Option<String>,
}

pub struct DriverHandler {
    addr: PciAddress,
    capabilities: Vec<(u8, PciCapability)>,

    state: Arc<State>,
}
fn with_pci_func_raw<T, F: FnOnce(&PciFunc) -> T>(pci: &dyn ConfigRegionAccess, addr: PciAddress, function: F) -> T {
    let func = PciFunc {
        pci,
        addr,
    };
    function(&func)
}
impl DriverHandler {
    fn respond(&mut self, request: driver_interface::PcidClientRequest, args: &driver_interface::SubdriverArguments) -> driver_interface::PcidClientResponse {
        use driver_interface::*;
        use crate::pci::cap::{MsiCapability, MsixCapability};

        match request {
            PcidClientRequest::RequestCapabilities => {
                PcidClientResponse::Capabilities(self.capabilities.iter().map(|(_, capability)| capability.clone()).collect::<Vec<_>>())
            }
            PcidClientRequest::RequestConfig => {
                PcidClientResponse::Config(args.clone())
            }
            PcidClientRequest::RequestFeatures => {
                PcidClientResponse::AllFeatures(self.capabilities.iter().filter_map(|(_, capability)| match capability {
                    PciCapability::Msi(msi) => Some((PciFeature::Msi, FeatureStatus::enabled(msi.enabled()))),
                    PciCapability::MsiX(msix) => Some((PciFeature::MsiX, FeatureStatus::enabled(msix.msix_enabled()))),
                    _ => None,
                }).collect())
            }
            PcidClientRequest::EnableFeature(feature) => match feature {
                PciFeature::Msi => {
                    let (offset, capability): (u8, &mut MsiCapability) = match self.capabilities.iter_mut().find_map(|&mut (offset, ref mut capability)| capability.as_msi_mut().map(|cap| (offset, cap))) {
                        Some(tuple) => tuple,
                        None => return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature)),
                    };
                    unsafe {
                        with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                            capability.set_enabled(true);
                            capability.write_message_control(func, offset);
                        });
                    }
                    PcidClientResponse::FeatureEnabled(feature)
                }
                PciFeature::MsiX => {
                    let (offset, capability): (u8, &mut MsixCapability) = match self.capabilities.iter_mut().find_map(|&mut (offset, ref mut capability)| capability.as_msix_mut().map(|cap| (offset, cap))) {
                        Some(tuple) => tuple,
                        None => return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature)),
                    };
                    unsafe {
                        with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                            capability.set_msix_enabled(true);
                            capability.write_a(func, offset);
                        });
                    }
                    PcidClientResponse::FeatureEnabled(feature)
                }
            }
            PcidClientRequest::FeatureStatus(feature) => PcidClientResponse::FeatureStatus(feature, match feature {
                PciFeature::Msi => self.capabilities.iter().find_map(|(_, capability)| if let PciCapability::Msi(msi) = capability {
                    Some(FeatureStatus::enabled(msi.enabled()))
                } else {
                    None
                }).unwrap_or(FeatureStatus::Disabled),
                PciFeature::MsiX => self.capabilities.iter().find_map(|(_, capability)| if let PciCapability::MsiX(msix) = capability {
                    Some(FeatureStatus::enabled(msix.msix_enabled()))
                } else {
                    None
                }).unwrap_or(FeatureStatus::Disabled),
            }),
            PcidClientRequest::FeatureInfo(feature) => PcidClientResponse::FeatureInfo(feature, match feature {
                PciFeature::Msi => if let Some(info) = self.capabilities.iter().find_map(|(_, capability)| capability.as_msi()) {
                    PciFeatureInfo::Msi(*info)
                } else {
                    return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature));
                }
                PciFeature::MsiX => if let Some(info) = self.capabilities.iter().find_map(|(_, capability)| capability.as_msix()) {
                    PciFeatureInfo::MsiX(*info)
                } else {
                    return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(feature));
                }
            }),
            PcidClientRequest::SetFeatureInfo(info_to_set) => match info_to_set {
                SetFeatureInfo::Msi(info_to_set) => if let Some((offset, info)) = self.capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msi_mut()?))) {
                    if let Some(mme) = info_to_set.multi_message_enable {
                        if info.multi_message_capable() < mme || mme > 0b101 {
                            return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                        }
                        info.set_multi_message_enable(mme);

                    }
                    if let Some(message_addr_and_data) = info_to_set.message_address_and_data {
                        let message_addr = message_addr_and_data.addr;
                        if message_addr & 0b11 != 0 {
                            return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                        }
                        info.set_message_address(message_addr as u32);
                        info.set_message_upper_address((message_addr >> 32) as u32);
                        if message_addr_and_data.data & ((1 << info.multi_message_enable()) - 1) != 0 {
                            return PcidClientResponse::Error(PcidServerResponseError::InvalidBitPattern);
                        }
                        info.set_message_data(
                            message_addr_and_data
                                .data
                                .try_into()
                                .expect("pcid: MSI message data too big"),
                        );
                    }
                    if let Some(mask_bits) = info_to_set.mask_bits {
                        info.set_mask_bits(mask_bits);
                    }
                    unsafe {
                        with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                            info.write_all(func, offset);
                        });
                    }
                    PcidClientResponse::SetFeatureInfo(PciFeature::Msi)
                } else {
                    return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(PciFeature::Msi));
                }
                SetFeatureInfo::MsiX { function_mask } => if let Some((offset, info)) = self.capabilities.iter_mut().find_map(|(offset, capability)| Some((*offset, capability.as_msix_mut()?))) {
                    if let Some(mask) = function_mask {
                        info.set_function_mask(mask);
                        unsafe {
                            with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                                info.write_a(func, offset);
                            });
                        }
                    }
                    PcidClientResponse::SetFeatureInfo(PciFeature::MsiX)
                } else {
                    return PcidClientResponse::Error(PcidServerResponseError::NonexistentFeature(PciFeature::MsiX));
                }
            }
            PcidClientRequest::ReadConfig(offset) => {
                let value = unsafe {
                    with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                        func.read_u32(offset)
                    })
                };
                return PcidClientResponse::ReadConfig(value);
            },
            PcidClientRequest::WriteConfig(offset, value) => {
                unsafe {
                    with_pci_func_raw(&self.state.pcie, self.addr, |func| {
                        func.write_u32(offset, value);
                    });
                }
                return PcidClientResponse::WriteConfig;
            }
        }
    }
    fn handle_spawn(mut self, pcid_to_client_write: usize, pcid_from_client_read: usize, args: driver_interface::SubdriverArguments) {
        use driver_interface::*;

        let mut pcid_to_client = unsafe { File::from_raw_fd(pcid_to_client_write as RawFd) };
        let mut pcid_from_client = unsafe { File::from_raw_fd(pcid_from_client_read as RawFd) };

        while let Ok(msg) = recv(&mut pcid_from_client) {
            let response = self.respond(msg, &args);
            send(&mut pcid_to_client, &response).unwrap();
        }
    }
}

pub struct State {
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
    pcie: Pcie,
}

fn print_pci_function(addr: PciAddress, header: &PciHeader) {
    let mut string = format!("PCI {} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
                             addr, header.vendor_id(), header.device_id(), header.class(),
                             header.subclass(), header.interface(), header.revision(), header.class());
    let device_type = DeviceType::from((header.class(), header.subclass()));
    match device_type {
        DeviceType::LegacyVgaCompatible => string.push_str("  VGA CTL"),
        DeviceType::IdeController => string.push_str(" IDE"),
        DeviceType::SataController => match header.interface() {
            0 => string.push_str(" SATA VND"),
            1 => string.push_str(" SATA AHCI"),
            _ => (),
        },
        DeviceType::UsbController => match header.interface() {
            0x00 => string.push_str(" UHCI"),
            0x10 => string.push_str(" OHCI"),
            0x20 => string.push_str(" EHCI"),
            0x30 => string.push_str(" XHCI"),
            _ => (),
        },
        _ => (),
    }
    info!("{}", string);
}

fn handle_parsed_header(state: Arc<State>, config: &Config, addr: PciAddress, header: PciEndpointHeader) {
    for driver in config.drivers.iter() {
        if !driver.match_function(header.full_device_id()) {
            continue;
        }

        let Some(ref args) = driver.command else {
            continue;
        };

        let mut string = String::new();
        let bars = header.bars(&state.pcie);
        for (i, bar) in bars.iter().enumerate() {
            if !bar.is_none() {
                string.push_str(&format!(" {i}={}", bar.display()));
            }
        }

        if !string.is_empty() {
            info!("    BAR{}", string);
        }

        let endpoint_header = header.endpoint_header(&state.pcie);

        // Enable bus mastering, memory space, and I/O space
        endpoint_header.update_command(&state.pcie, |cmd| {
            cmd | CommandRegister::BUS_MASTER_ENABLE
                | CommandRegister::MEMORY_ENABLE
                | CommandRegister::IO_ENABLE
        });

        // Set IRQ line to 9 if not set
        let mut irq;
        let interrupt_pin;

        unsafe {
            let mut data = state.pcie.read(addr, 0x3C);
            irq = (data & 0xFF) as u8;
            interrupt_pin = ((data & 0x0000_FF00) >> 8) as u8;
            if irq == 0xFF {
                irq = 9;
            }
            data = (data & 0xFFFFFF00) | irq as u32;
            state.pcie.write(addr, 0x3C, data);
        };

        let legacy_interrupt_enabled = match interrupt_pin {
            0 => false,
            1 | 2 | 3 | 4 => true,

            other => {
                warn!("pcid: invalid interrupt pin: {}", other);
                false
            }
        };

        let capabilities = if endpoint_header.status(&state.pcie).has_capability_list() {
            let func = PciFunc {
                pci: &state.pcie,
                addr
            };
            crate::pci::cap::CapabilitiesIter { inner: crate::pci::cap::CapabilityOffsetsIter::new(header.cap_pointer(), &func) }.collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        debug!("PCI DEVICE CAPABILITIES for {}: {:?}", args.iter().map(|string| string.as_ref()).nth(0).unwrap_or("[unknown]"), capabilities);

        let func = driver_interface::PciFunction {
            bars,
            addr,
            legacy_interrupt_line: if legacy_interrupt_enabled {
                Some(LegacyInterruptLine(irq))
            } else {
                None
            },
            full_device_id: header.full_device_id().clone(),
        };

        let subdriver_args = driver_interface::SubdriverArguments {
            func,
        };

        let mut args = args.iter();
        if let Some(program) = args.next() {
            let mut command = Command::new(program);
            for arg in args {
                if arg.starts_with("$") {
                    panic!("support for $VARIABLE has been removed. use pcid_interface instead");
                }
                command.arg(arg);
            }

            info!("PCID SPAWN {:?}", command);

            // TODO: libc wrapper?
            let [fds1, fds2] = unsafe {
                let mut fds1 = [0 as libc::c_int; 2];
                let mut fds2 = [0 as libc::c_int; 2];

                assert_eq!(libc::pipe(fds1.as_mut_ptr()), 0, "pcid: failed to create pcid->client pipe");
                assert_eq!(libc::pipe(fds2.as_mut_ptr()), 0, "pcid: failed to create client->pcid pipe");

                [
                    fds1.map(|c| c as usize),
                    fds2.map(|c| c as usize),
                ]
            };

            let [pcid_to_client_read, pcid_to_client_write] = fds1;
            let [pcid_from_client_read, pcid_from_client_write] = fds2;

            let envs = vec![
                ("PCID_TO_CLIENT_FD", format!("{}", pcid_to_client_read)),
                ("PCID_FROM_CLIENT_FD", format!("{}", pcid_from_client_write)),
            ];

            match command.envs(envs).spawn() {
                Ok(mut child) => {
                    let driver_handler = DriverHandler {
                        addr,
                        state: Arc::clone(&state),
                        capabilities,
                    };
                    let _handle = thread::spawn(move || {
                        driver_handler.handle_spawn(pcid_to_client_write, pcid_from_client_read, subdriver_args);
                    });
                    // FIXME this currently deadlocks as pcid doesn't daemonize
                    //state.threads.lock().unwrap().push(handle);
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

    #[cfg(target_os = "redox")] {
        match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.log") {
            Ok(b) => logger = logger.with_output(
                b.with_filter(log::LevelFilter::Trace)
                    .flush_on_newline(true)
                    .build()
            ),
            Err(error) => eprintln!("pcid: failed to open pcid.log"),
        }
        match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.ansi.log") {
            Ok(b) => logger = logger.with_output(
                b.with_filter(log::LevelFilter::Trace)
                    .with_ansi_escape_codes()
                    .flush_on_newline(true)
                    .build()
            ),
            Err(error) => eprintln!("pcid: failed to open pcid.ansi.log"),
        }
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

#[paw::main]
fn main(args: Args) {
    let mut config = Config::default();

    if let Some(config_path) = args.config_path {
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

    let _logger_ref = setup_logging(args.verbose);

    let state = Arc::new(State {
        pcie: Pcie::new(),
        threads: Mutex::new(Vec::new()),
    });

    info!("PCI SG-BS:DV.F VEND:DEVI CL.SC.IN.RV");

    // FIXME Use full ACPI for enumerating the host bridges. MCFG only describes the first
    // host bridge, while multi-processor systems likely have a host bridge for each CPU.
    // See also https://www.kernel.org/doc/html/latest/PCI/acpi-info.html
    let mut bus_nums = vec![0];
    let mut bus_i = 0;
    while bus_i < bus_nums.len() {
        let bus_num = bus_nums[bus_i];
        bus_i += 1;

        'dev: for dev_num in 0..32 {
            for func_num in 0..8 {
                let func_addr = PciAddress::new(0, bus_num, dev_num, func_num);
                match PciHeader::from_reader(&state.pcie, func_addr) {
                    Ok(header) => {
                        print_pci_function(func_addr, &header);
                        match header {
                            PciHeader::General(endpoint_header) => {
                                handle_parsed_header(
                                    Arc::clone(&state),
                                    &config,
                                    func_addr,
                                    endpoint_header,
                                );
                            }
                            PciHeader::PciToPci {
                                secondary_bus_num, ..
                            } => {
                                bus_nums.push(secondary_bus_num);
                            }
                        }
                    }
                    Err(PciHeaderError::NoDevice) => {
                        if func_addr.function() == 0 {
                                trace!("PCI {:>02X}:{:>02X}: no dev", bus_num, dev_num);
                                continue 'dev;
                        }
                    },
                    Err(PciHeaderError::UnknownHeaderType(id)) => {
                        warn!("pcid: unknown header type: {id:?}");
                    }
                }
            }
        }
    }

    for thread in state.threads.lock().unwrap().drain(..) {
        thread.join().unwrap();
    }
}
