use std::fs::{self, File};
use std::io::prelude::*;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use pcid_lib::driver_interface::PciClass;

mod config;
use self::config::{Config, DriverConfig};

fn main() -> Result<()> {
    let mut args = pico_args::Arguments::from_env();
    let config_path = args.opt_value_from_str::<_, String>("--config").expect("failed to parse --config argument");

    let _ = setup_logging();

    let mut config = Config::default();

    if let Some(config_path) = config_path {
        if fs::metadata(&config_path)?.is_file() {
            if let Ok(mut config_file) = File::open(&config_path) {
                let mut config_data = String::new();
                if let Ok(_) = config_file.read_to_string(&mut config_data) {
                    config = toml::from_str(&config_data).unwrap_or(Config::default());
                }
            }
        } else {
            let paths = fs::read_dir(&config_path)?;

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

    for entry in fs::read_dir("pci:tree")? {
        let entry = entry.context("failed to get entry")?;
        log::trace!("ENTRY: {}", entry.path().to_string_lossy());
        let addr = entry.file_name().to_string_lossy().parse::<pcid_lib::PciAddr>().context("malformed PCI address")?;
        let path = entry.path();

        let r = |name| fs::read_to_string(path.join(name)).context("failed to read PCI device property");
        let w = |name, v| fs::write(path.join(name), v).context("failed to write to PCI device directory");

        let header = Header {
            vendor_id: u16::from_str_radix(&r("vendor-id")?, 16)?,
            device_id: u16::from_str_radix(&r("device-id")?, 16)?,
            class: r("class")?.parse::<u8>()?.into(),
            subclass: r("subclass")?.parse::<u8>()?,
            interface: r("interface")?.parse::<u8>()?,
            irq: r("interrupt-line")?.parse::<u8>()?,
            revision: r("revision")?.parse::<u8>()?,
            bars: r("bars")?.lines().map(|line| {
                let bar_str = line.trim();
                // TODO: better way to pass the BARs from the pci scheme
                Ok(if bar_str == "None" {
                    pcid_lib::driver_interface::PciBar::None
                } else if bar_str.len() == 4 {
                    pcid_lib::driver_interface::PciBar::Port(u16::from_str_radix(bar_str, 16)?)
                } else if bar_str.len() == 8 {
                    pcid_lib::driver_interface::PciBar::Memory(u32::from_str_radix(bar_str, 16)?)
                } else {
                    bail!("invalid BAR string length");
                })
            }).collect::<Result<Vec<_>>>()?.try_into().map_err(|_| anyhow!("invalid number of BARs"))?,
            bar_sizes: r("bar-sizes")?.lines().map(|l| Ok(u32::from_str_radix(l, 16)?)).collect::<Result<Vec<_>>>()?.try_into().map_err(|_| anyhow!("invalid number of BAR sizes"))?,
        };

        let (driver, args) = match find_driver(&config, &header, addr).context("failed to find driver")? {
            Some(d) => d,
            None => {
                log::debug!("no driver for {}, continuing", addr);
                continue;
            }
        };
        w("enabled", "1").context("failed to enable device")?;
        spawn_driver(addr, &header, driver, args).context("failed to spawn driver")?;
    }

    Ok(())
}
fn setup_logging() -> Option<&'static redox_log::RedoxLogger> {
    use redox_log::*;

    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_ansi_escape_codes()
                .with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
         );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid-spawner.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Trace)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("pcid-spawner: failed to open pcid-spawner.log"),
    }
    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("bus", "pci", "pcid-spawner.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Trace)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("pcid-spawner: failed to open pcid-spawner.ansi.log"),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("pcid-spawner: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("pcid-spawner: failed to set default logger: {}", error);
            None
        }
    }
}
struct Header {
    vendor_id: u16,
    device_id: u16,
    class: PciClass,
    subclass: u8,
    interface: u8,
    irq: u8,
    revision: u8,
    bars: [pcid_lib::driver_interface::PciBar; 6],
    bar_sizes: [u32; 6],
}
fn spawn_driver(addr: pcid_lib::PciAddr, header: &Header, driver: &DriverConfig, args: &[String]) -> Result<()> {
    let mut args = args.iter();

    let program = args.next().ok_or_else(|| anyhow!("driver configuration entry did not have any command!"))?;

    let mut command = Command::new(program);
    for arg in args {
        let arg = match arg.as_str() {
            "$BUS" => format!("{:>02X}", addr.bus),
            "$DEV" => format!("{:>02X}", addr.dev),
            "$FUNC" => format!("{:>02X}", addr.func),
            "$NAME" => format!("pci-{}", addr),
            "$BAR0" => format!("{}", header.bars[0]),
            "$BAR1" => format!("{}", header.bars[1]),
            "$BAR2" => format!("{}", header.bars[2]),
            "$BAR3" => format!("{}", header.bars[3]),
            "$BAR4" => format!("{}", header.bars[4]),
            "$BAR5" => format!("{}", header.bars[5]),
            "$BARSIZE0" => format!("{:>08X}", header.bar_sizes[0]),
            "$BARSIZE1" => format!("{:>08X}", header.bar_sizes[1]),
            "$BARSIZE2" => format!("{:>08X}", header.bar_sizes[2]),
            "$BARSIZE3" => format!("{:>08X}", header.bar_sizes[3]),
            "$BARSIZE4" => format!("{:>08X}", header.bar_sizes[4]),
            "$BARSIZE5" => format!("{:>08X}", header.bar_sizes[5]),
            "$IRQ" => format!("{}", header.irq),
            "$VENID" => format!("{:>04X}", header.vendor_id),
            "$DEVID" => format!("{:>04X}", header.device_id),

            _ => arg.clone(),
        };
        command.arg(&arg);
    }

    log::info!("PCID_SPAWNER SPAWN {:?}", command);

    let envs = if driver.use_channel.unwrap_or(false) {
        let channel_fd = syscall::open(&format!("pci:tree/{}/channel", addr), syscall::O_RDWR).map_err(|err| anyhow!("failed to open pcid channel: {}", err))?;
        vec! [("PCID_CLIENT_CHANNEL", format!("{}", channel_fd))]
    } else {
        vec! []
    };

    match command.envs(envs).spawn() {
        Ok(mut child) => match child.wait() {
            Ok(_status) => (),
            Err(err) => log::error!("pcid: failed to wait for {:?}: {}", command, err),
        }
        Err(err) => log::error!("pcid: failed to execute {:?}: {}", command, err)
    }
    Ok(())
}
fn find_driver<'config>(config: &'config Config, header: &Header, addr: pcid_lib::PciAddr) -> Result<Option<(&'config DriverConfig, &'config [String])>> {
    let raw_class: u8 = header.class.into();
    let mut string = format!("PCI {:>04X}/{:>02X}/{:>02X}/{:>02X} {:>04X}:{:>04X} {:>02X}.{:>02X}.{:>02X}.{:>02X} {:?}",
                             addr.seg, addr.bus, addr.dev, addr.func, header.vendor_id, header.device_id, raw_class,
                             header.subclass, header.interface, header.revision, header.class);

    match header.class {
        PciClass::Legacy if header.subclass == 1 => string.push_str("  VGA CTL"),
        PciClass::Storage => match header.subclass {
            0x01 => {
                string.push_str(" IDE");
            },
            0x06 => if header.interface == 0 {
                string.push_str(" SATA VND");
            } else if header.interface == 1 {
                string.push_str(" SATA AHCI");
            },
            _ => ()
        },
        PciClass::SerialBus => match header.subclass {
            0x03 => match header.interface {
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

    for (i, bar) in header.bars.iter().enumerate() {
        if !bar.is_none() {
            string.push_str(&format!(" {}={}", i, bar));
        }
    }

    log::debug!("pcid-spawner enumerated: {}", string);

    for driver in config.drivers.iter() {
        if driver.class.map_or(false, |c| c != raw_class) { continue; }
        if driver.subclass.map_or(false, |s| s != header.subclass) { continue; }
        if driver.interface.map_or(false, |i| i != header.interface) { continue; }

        if let Some(ref ids) = driver.ids {
            let device_found = ids
                .iter()
                .filter_map(|(vendor, devices)| u16::from_str_radix(vendor.trim_start_matches("0x"), 16).ok().map(|v| (v, devices)))
                .any(|(vendor, devices)| vendor == header.vendor_id && devices.iter().any(|&d| d == header.device_id));

            if !device_found { continue; }
        } else {
            if driver.vendor.map_or(false, |v| v != header.vendor_id) { continue; }
            if driver.device.map_or(false, |d| d != header.device_id) { continue; }
        }

        if let Some(ref device_id_range) = driver.device_id_range {
            if header.device_id < device_id_range.start  ||
               device_id_range.end <= header.device_id { continue; }
        }
        let args = match driver.command {
            Some(ref a) => a,
            None => continue,
        };

        return Ok(Some((driver, args)));
    }
    Ok(None)
}
