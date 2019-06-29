#![feature(asm)]

#[macro_use] extern crate bitflags;
extern crate byteorder;
#[macro_use] extern crate serde_derive;
extern crate syscall;
extern crate toml;

use std::env;
use std::fs::File;
use std::io::Read;
use std::process::Command;
use syscall::iopl;

use crate::config::Config;
use crate::pci::{Pci, PciClass, PciHeader, PciHeaderError, PciHeaderType};

mod config;
mod pci;

fn handle_parsed_header(config: &Config, pci: &Pci, bus_num: u8,
                        dev_num: u8, func_num: u8, header: PciHeader) {
    let raw_class: u8 = header.class().into();
    let mut string = format!("PCI {:>02X}/{:>02X}/{:>02X} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
                             bus_num, dev_num, func_num, header.vendor_id(), header.device_id(), raw_class,
                             header.subclass(), header.interface(), header.revision(), header.class());

    match header.class() {
        PciClass::Storage => match header.subclass() {
            0x01 => {
                string.push_str(" IDE");
            },
            0x06 => {
                string.push_str(" SATA");
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

        if let Some(vendor) = driver.vendor {
            if vendor != header.vendor_id() { continue; }
        }

        if let Some(device) = driver.device {
            if device != header.device_id() { continue; }
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
                        "$BAR0" => format!("{}", header.get_bar(0)),
                        "$BAR1" => format!("{}", header.get_bar(1)),
                        "$BAR2" if header.header_type() == PciHeaderType::GENERAL =>
                            format!("{}", header.get_bar(2)),
                        "$BAR3" if header.header_type() == PciHeaderType::GENERAL =>
                            format!("{}", header.get_bar(3)),
                        "$BAR4" if header.header_type() == PciHeaderType::GENERAL =>
                            format!("{}", header.get_bar(4)),
                        "$BAR5" if header.header_type() == PciHeaderType::GENERAL =>
                            format!("{}", header.get_bar(5)),
                        "$IRQ" => format!("{}", irq),
                        "$VENID" => format!("{:>04X}", header.vendor_id()),
                        "$DEVID" => format!("{:>04X}", header.device_id()),
                        _ => arg.clone()
                    };
                    command.arg(&arg);
                }

                println!("PCID SPAWN {:?}", command);
                match command.spawn() {
                    Ok(mut child) => match child.wait() {
                        Ok(_status) => (), //println!("pcid: waited for {}: {:?}", line, status.code()),
                        Err(err) => println!("pcid: failed to wait for {:?}: {}", command, err)
                    },
                    Err(err) => println!("pcid: failed to execute {:?}: {}", command, err)
                }
            }
        }
    }
}

fn main() {
    let mut config = Config::default();

    let mut args = env::args().skip(1);
    if let Some(config_path) = args.next() {
        if let Ok(mut config_file) = File::open(&config_path) {
            let mut config_data = String::new();
            if let Ok(_) = config_file.read_to_string(&mut config_data) {
                config = toml::from_str(&config_data).unwrap_or(Config::default());
            }
        }
    }

    unsafe { iopl(3).unwrap() };

    print!("PCI BS/DV/FN VEND:DEVI CL.SC.IN.RV\n");

    let pci = Pci::new();
    for bus in pci.buses() {
        for dev in bus.devs() {
            for func in dev.funcs() {
                let func_num = func.num;
                match PciHeader::from_reader(func) {
                    Ok(header) => {
                        handle_parsed_header(&config, &pci, bus.num, dev.num, func_num, header);
                    }
                    Err(PciHeaderError::NoDevice) => {},
                    Err(PciHeaderError::UnknownHeaderType(id)) => {
                        println!("pcid: unknown header type: {}", id);
                    }
                }
            }
        }
    }
}
