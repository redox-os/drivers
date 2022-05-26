use std::fs::{self, File};
use std::io::prelude::*;

mod config;
use self::config::Config;

fn main() {
    let mut args = pico_args::Arguments::from_env();
    let config_path = args.opt_value_from_str::<_, String>("--config").expect("failed to parse --config argument");

    let mut config = Config::default();

    if let Some(config_path) = config_path {
        if fs::metadata(&config_path).unwrap().is_file() {
            if let Ok(mut config_file) = File::open(&config_path) {
                let mut config_data = String::new();
                if let Ok(_) = config_file.read_to_string(&mut config_data) {
                    config = toml::from_str(&config_data).unwrap_or(Config::default());
                }
            }
        } else {
            let paths = fs::read_dir(&config_path).unwrap();

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

    for addr in fs::read_dir("pci:tree").unwrap() {
        let addr = addr.unwrap();
    }
}
fn f() {
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

        let envs = if driver.use_channel.unwrap_or(false) {
            vec! [("PCID_CLIENT_CHANNEL", format!("{}", channel)
        } else {
            (None, None, vec! [])
        };

        match command.envs(envs).spawn() {
            Ok(mut child) => match child.wait() {
                Ok(_status) => (),
                Err(err) => error!("pcid: failed to wait for {:?}: {}", command, err),
            }
            Err(err) => error!("pcid: failed to execute {:?}: {}", command, err)
        }
    }
}
fn g() {
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
}
fn h() {
    let raw_class: u8 = header.class().into();
    let mut string = format!("PCI {:>02X}/{:>02X}/{:>02X} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
                             bus_num, dev_num, func_num, header.vendor_id(), header.device_id(), raw_class,
                             header.subclass(), header.interface(), header.revision(), header.class());

}
