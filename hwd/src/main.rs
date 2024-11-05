use amlserde::{AmlSerde, AmlSerdeValue};
use std::{error::Error, fs};

fn acpi() -> Result<(), Box<dyn Error>> {
    for entry_res in fs::read_dir("/scheme/acpi/symbols")? {
        let entry = entry_res?;
        if let Some(file_name) = entry.file_name().to_str() {
            if file_name.ends_with("_CID") || file_name.ends_with("_HID") {
                let ron = fs::read_to_string(entry.path())?;
                let AmlSerde { name, value } = ron::from_str(&ron)?;
                let id = match value {
                    AmlSerdeValue::Integer(integer) => {
                        let vendor = integer & 0xFFFF;
                        let device = (integer >> 16) & 0xFFFF;
                        let vendor_rev = ((vendor & 0xFF) << 8) | vendor >> 8;
                        let vendor_1 = (((vendor_rev >> 10) & 0x1f) as u8 + 64) as char;
                        let vendor_2 = (((vendor_rev >> 5) & 0x1f) as u8 + 64) as char;
                        let vendor_3 = (((vendor_rev >> 0) & 0x1f) as u8 + 64) as char;
                        format!("{}{}{}{:04X}", vendor_1, vendor_2, vendor_3, device)
                    }
                    AmlSerdeValue::String(string) => {
                        string
                    },
                    _ => {
                        log::warn!("{}: unsupported value {:x?}", name, value);
                        continue;
                    }
                };
                log::debug!("{}: {}", name, id);
            }
        }
    }
    Ok(())
}

fn main() {
    common::setup_logging(
        "misc",
        "hwd",
        "hwd",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    //TODO: HWD is meant to locate PCI/XHCI/etc devices in ACPI and DeviceTree definitions and start their drivers
    acpi().unwrap();
}
