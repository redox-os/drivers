#![feature(asm)]

extern crate bitflags;
extern crate byteorder;
extern crate syscall;
extern crate toml;

use std::{env, io, i64};
use std::fs::{File, metadata, read_dir};
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::Command;
use syscall::iopl;

use std::os::unix::process::CommandExt;

use crate::config::Config;
use crate::pci::{Pci, PciBar, PciClass, PciHeader, PciHeaderError, PciHeaderType};

mod config;
mod driver_interface;
mod pci;

fn handle_parsed_header(config: &Config, pci: &Pci, bus_num: u8,
                        dev_num: u8, func_num: u8, header: PciHeader) {
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

    string.push('\n');

    print!("{}", string);

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
            unsafe {
                let mut data = pci.read(bus_num, dev_num, func_num, 0x3C);
                irq = (data & 0xFF) as u8;
                if irq == 0xFF {
                    irq = 9;
                }
                data = (data & 0xFFFFFF00) | irq as u32;
                pci.write(bus_num, dev_num, func_num, 0x3C, data);
            }

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

                    let original = pci.read(bus_num, dev_num, func_num, offset);
                    pci.write(bus_num, dev_num, func_num, offset, 0xFFFFFFFF);

                    let new = pci.read(bus_num, dev_num, func_num, offset);
                    pci.write(bus_num, dev_num, func_num, offset, original);

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

            let func = driver_interface::PciFunction {
                bars,
                bar_sizes,
                bus_num,
                dev_num,
                func_num,
                devid: header.device_id(),
                legacy_interrupt_line: irq,
                venid: header.vendor_id(),
            };
            let capabilities = Vec::new();

            let subdriver_args = driver_interface::SubdriverArguments {
                capabilities,
                func,
            };

            // TODO: find a better way to pass the header data down to the
            // device driver, making passing the capabilities list etc
            // posible.
            let mut args = args.iter();
            if let Some(program) = args.next() {
                let mut command = Command::new(program);
                for arg in args {
                    let arg = match arg.as_str() {
                        "$BUS" => format!("{:>02X}", bus_num),
                        "$DEV" => format!("{:>02X}", dev_num),
                        "$FUNC" => format!("{:>02X}", func_num),
                        "$NAME" => format!("pci-{:>02X}.{:>02X}.{:>02X}", bus_num, dev_num, func_num),
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

                println!("PCID SPAWN {:?}", command);

                let (pcid_to_client_write, pcid_from_client_read, envs) = if driver.channel_name.is_some() {
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
                        handle_spawn(pcid_to_client_write, pcid_from_client_read, subdriver_args);
                        match child.wait() {
                            Ok(_status) => (),
                            Err(err) => println!("pcid: failed to wait for {:?}: {}", command, err),
                        }
                    }
                    Err(err) => println!("pcid: failed to execute {:?}: {}", command, err)
                }
            }
        }
    }
    fn handle_spawn(pcid_to_client_write: Option<usize>, pcid_from_client_read: Option<usize>, args: driver_interface::SubdriverArguments) {
        use driver_interface::*;

        // TODO: Instead of relying on the subdriver to correctly close the pipe, there should be a
        // dedicated thread responsible for this. Or alternatively, a thread pool with Futures.

        if let (Some(pcid_to_client_fd), Some(pcid_from_client_fd)) = (pcid_to_client_write, pcid_from_client_read) {
            let mut pcid_to_client = unsafe { File::from_raw_fd(pcid_to_client_fd as RawFd) };
            let mut pcid_from_client = unsafe { File::from_raw_fd(pcid_from_client_fd as RawFd) };

            if let Ok(msg) = recv(&mut pcid_from_client) {
                match msg {
                    PcidClientRequest::RequestConfig => {
                        send(&mut pcid_to_client, &PcidClientResponse::Config(args.clone())).unwrap();
                    }
                }
            }
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

    unsafe { iopl(3).unwrap() };

    print!("PCI BS/DV/FN VEND:DEVI CL.SC.IN.RV\n");

    let pci = Pci::new();
    'bus: for bus in pci.buses() {
        'dev: for dev in bus.devs() {
            for func in dev.funcs() {
                let func_num = func.num;
                match PciHeader::from_reader(func) {
                    Ok(header) => {
                        handle_parsed_header(&config, &pci, bus.num, dev.num, func_num, header);
                    }
                    Err(PciHeaderError::NoDevice) => {
                        if func_num == 0 {
                            if dev.num == 0 {
                                // println!("PCI {:>02X}: no bus", bus.num);
                                continue 'bus;
                            } else {
                                // println!("PCI {:>02X}/{:>02X}: no dev", bus.num, dev.num);
                                continue 'dev;
                            }
                        }
                    },
                    Err(PciHeaderError::UnknownHeaderType(id)) => {
                        println!("pcid: unknown header type: {}", id);
                    }
                }
            }
        }
    }
}
