#![feature(non_exhaustive_omitted_patterns_lint)]
#![feature(iter_next_chunk)]
#![feature(if_let_guard)]

use std::fs::{metadata, read_dir, File};
use std::io::prelude::*;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{debug, info, trace, warn};
use pci_types::{
    Bar as TyBar, CommandRegister, EndpointHeader, HeaderType, PciAddress,
    PciHeader as TyPciHeader, PciPciBridgeHeader,
};
use structopt::StructOpt;

use crate::cfg_access::Pcie;
use crate::config::Config;
use pcid_interface::{FullDeviceId, LegacyInterruptLine, PciBar};

mod cfg_access;
mod config;
mod driver_handler;

#[derive(StructOpt)]
#[structopt(about)]
struct Args {
    #[structopt(
        short,
        long,
        help = "Increase logging level once for each arg.",
        parse(from_occurrences)
    )]
    verbose: u8,

    #[structopt(
        help = "A path to a pcid config file or a directory that contains pcid config files."
    )]
    config_path: Option<String>,
}

pub struct State {
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
    pcie: Pcie,
}

fn handle_parsed_header(
    state: Arc<State>,
    config: &Config,
    mut endpoint_header: EndpointHeader,
    full_device_id: FullDeviceId,
) {
    for driver in config.drivers.iter() {
        if !driver.match_function(&full_device_id) {
            continue;
        }

        let Some(ref args) = driver.command else {
            continue;
        };

        let mut bars = [PciBar::None; 6];
        let mut skip = false;
        for i in 0..6 {
            if skip {
                skip = false;
                continue;
            }
            match endpoint_header.bar(i, &state.pcie) {
                Some(TyBar::Io { port }) => {
                    bars[i as usize] = PciBar::Port(port.try_into().unwrap())
                }
                Some(TyBar::Memory32 {
                    address,
                    size,
                    prefetchable: _,
                }) => {
                    bars[i as usize] = PciBar::Memory32 {
                        addr: address,
                        size,
                    }
                }
                Some(TyBar::Memory64 {
                    address,
                    size,
                    prefetchable: _,
                }) => {
                    bars[i as usize] = PciBar::Memory64 {
                        addr: address,
                        size,
                    };
                    skip = true; // Each 64bit memory BAR occupies two slots
                }
                None => bars[i as usize] = PciBar::None,
            }
        }

        let mut string = String::new();
        for (i, bar) in bars.iter().enumerate() {
            if !bar.is_none() {
                string.push_str(&format!(" {i}={}", bar.display()));
            }
        }

        if !string.is_empty() {
            info!("    BAR{}", string);
        }

        // Enable bus mastering, memory space, and I/O space
        endpoint_header.update_command(&state.pcie, |cmd| {
            cmd | CommandRegister::BUS_MASTER_ENABLE
                | CommandRegister::MEMORY_ENABLE
                | CommandRegister::IO_ENABLE
        });

        // Set IRQ line to 9 if not set
        let mut irq = 0xFF;
        let mut interrupt_pin = 0xFF;

        endpoint_header.update_interrupt(&state.pcie, |(pin, mut line)| {
            if line == 0xFF {
                line = 9;
            }
            irq = line;
            interrupt_pin = pin;
            (pin, line)
        });

        let legacy_interrupt_enabled = match interrupt_pin {
            0 => false,
            1 | 2 | 3 | 4 => true,

            other => {
                warn!("pcid: invalid interrupt pin: {}", other);
                false
            }
        };

        let mut phandled: Option<(u32, [u32; 3])> = None;
        if legacy_interrupt_enabled {
            let pci_address = endpoint_header.header().address();
            let dt_address = ((pci_address.bus() as u32) << 16)
                | ((pci_address.device() as u32) << 11)
                | ((pci_address.function() as u32) << 8);
            let addr = [
                dt_address & state.pcie.interrupt_map_mask[0],
                0u32,
                0u32,
                interrupt_pin as u32 & state.pcie.interrupt_map_mask[3],
            ];
            let mapping = state
                .pcie
                .interrupt_map
                .iter()
                .find(|x| x.addr == addr[0..3] && x.interrupt == addr[3]);
            if let Some(mapping) = mapping {
                debug!(
                    "found mapping: addr={:?} => (phandle={} irq={:?})",
                    addr, mapping.parent_phandle, mapping.parent_interrupt
                );
                phandled = Some((mapping.parent_phandle, mapping.parent_interrupt));
            }
        }

        let capabilities = if endpoint_header.status(&state.pcie).has_capability_list() {
            endpoint_header
                .capabilities(&state.pcie)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        debug!(
            "PCI DEVICE CAPABILITIES for {}: {:?}",
            args.iter()
                .map(|string| string.as_ref())
                .nth(0)
                .unwrap_or("[unknown]"),
            capabilities
        );

        let func = pcid_interface::PciFunction {
            bars,
            addr: endpoint_header.header().address(),
            legacy_interrupt_line: if legacy_interrupt_enabled {
                Some(LegacyInterruptLine { irq, phandled })
            } else {
                None
            },
            full_device_id: full_device_id.clone(),
        };

        driver_handler::DriverHandler::spawn(Arc::clone(&state), func, capabilities, args);
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

    let log_level = match args.verbose {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };

    common::setup_logging("bus", "pci", "pcid", log_level, log::LevelFilter::Trace);

    redox_daemon::Daemon::new(move |daemon| main_inner(config, daemon)).unwrap();
}

fn main_inner(config: Config, daemon: redox_daemon::Daemon) -> ! {
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
                let header = TyPciHeader::new(PciAddress::new(0, bus_num, dev_num, func_num));

                let (vendor_id, device_id) = header.id(&state.pcie);
                if vendor_id == 0xffff && device_id == 0xffff {
                    if func_num == 0 {
                        trace!("PCI {:>02X}:{:>02X}: no dev", bus_num, dev_num);
                        continue 'dev;
                    }

                    continue;
                }

                let (revision, class, subclass, interface) = header.revision_and_class(&state.pcie);
                let full_device_id = FullDeviceId {
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    interface,
                    revision,
                };

                info!("PCI {} {}", header.address(), full_device_id.display());

                match header.header_type(&state.pcie) {
                    HeaderType::Endpoint => {
                        handle_parsed_header(
                            Arc::clone(&state),
                            &config,
                            EndpointHeader::from_header(header, &state.pcie).unwrap(),
                            full_device_id,
                        );
                    }
                    HeaderType::PciPciBridge => {
                        let bridge_header =
                            PciPciBridgeHeader::from_header(header, &state.pcie).unwrap();
                        bus_nums.push(bridge_header.secondary_bus_number(&state.pcie));
                    }
                    ty => {
                        warn!("pcid: unknown header type: {ty:?}");
                    }
                }
            }
        }
    }

    daemon.ready().unwrap();

    for thread in state.threads.lock().unwrap().drain(..) {
        thread.join().unwrap();
    }

    std::process::exit(0);
}
