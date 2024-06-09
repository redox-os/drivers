use std::fs::{metadata, read_dir, File};
use std::io::prelude::*;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{debug, info, trace, warn};
use pci_types::{CommandRegister, PciAddress};
use redox_log::{OutputBuilder, RedoxLogger};
use structopt::StructOpt;

use crate::cfg_access::Pcie;
use crate::config::Config;
use crate::driver_interface::LegacyInterruptLine;
use crate::pci::PciFunc;
use crate::pci_header::{PciEndpointHeader, PciHeader, PciHeaderError};

mod cfg_access;
mod config;
mod driver_handler;
mod driver_interface;
mod pci;
mod pci_header;

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

fn handle_parsed_header(state: Arc<State>, config: &Config, header: PciEndpointHeader) {
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

        let mut endpoint_header = header.endpoint_header(&state.pcie);

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

        let capabilities = if endpoint_header.status(&state.pcie).has_capability_list() {
            let func = PciFunc {
                pci: &state.pcie,
                addr: header.address(),
            };
            crate::pci::cap::CapabilitiesIter::new(header.cap_pointer(), &func).collect::<Vec<_>>()
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

        let func = driver_interface::PciFunction {
            bars,
            addr: header.address(),
            legacy_interrupt_line: if legacy_interrupt_enabled {
                Some(LegacyInterruptLine(irq))
            } else {
                None
            },
            full_device_id: header.full_device_id().clone(),
        };

        driver_handler::DriverHandler::spawn(Arc::clone(&state), func, capabilities, args);
    }
}

fn setup_logging(verbosity: u8) -> Option<&'static RedoxLogger> {
    let log_level = match verbosity {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    let mut logger = RedoxLogger::new().with_output(
        OutputBuilder::stderr()
            .with_ansi_escape_codes()
            .with_filter(log_level)
            .flush_on_newline(true)
            .build(),
    );

    #[cfg(target_os = "redox")]
    {
        match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.log") {
            Ok(b) => {
                logger = logger.with_output(
                    b.with_filter(log::LevelFilter::Trace)
                        .flush_on_newline(true)
                        .build(),
                )
            }
            Err(error) => eprintln!("pcid: failed to open pcid.log"),
        }
        match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid.ansi.log") {
            Ok(b) => {
                logger = logger.with_output(
                    b.with_filter(log::LevelFilter::Trace)
                        .with_ansi_escape_codes()
                        .flush_on_newline(true)
                        .build(),
                )
            }
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
                        info!("{}", header.display());
                        match header {
                            PciHeader::General(endpoint_header) => {
                                handle_parsed_header(Arc::clone(&state), &config, endpoint_header);
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
                    }
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
