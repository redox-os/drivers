use amlserde::{AmlSerde, AmlSerdeValue};
use std::{error::Error, fs, process::Command};

use super::Backend;

pub struct AcpiBackend {
    rxsdt: Vec<u8>,
}

impl Backend for AcpiBackend {
    fn new() -> Result<Self, Box<dyn Error>> {
        let rxsdt = fs::read("/scheme/kernel.acpi/rxsdt")?;

        // Spawn acpid
        //TODO: pass rxsdt data to acpid?
        Command::new("acpid").spawn()?.wait()?;

        Ok(Self { rxsdt })
    }

    fn probe(&mut self) -> Result<(), Box<dyn Error>> {
        // Read symbols from acpi scheme
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
                            //TODO: simplify this nibble swap
                            let device_1 = (device >> 4) & 0xF;
                            let device_2 = (device >> 0) & 0xF;
                            let device_3 = (device >> 12) & 0xF;
                            let device_4 = (device >> 8) & 0xF;
                            format!(
                                "{}{}{}{:01X}{:01X}{:01X}{:01X}",
                                vendor_1,
                                vendor_2,
                                vendor_3,
                                device_1,
                                device_2,
                                device_3,
                                device_4
                            )
                        }
                        AmlSerdeValue::String(string) => string,
                        _ => {
                            log::warn!("{}: unsupported value {:x?}", name, value);
                            continue;
                        }
                    };
                    let what = match id.as_str() {
                        // https://uefi.org/specs/ACPI/6.5/05_ACPI_Software_Programming_Model.html
                        "ACPI0003" => "Power source",
                        "ACPI0006" => "GPE block",
                        "ACPI0007" => "Processor",
                        "ACPI0010" => "Processor control",
                        // https://uefi.org/sites/default/files/resources/devids%20%285%29.txt
                        "PNP0000" => "AT interrupt controller",
                        "PNP0100" => "AT timer",
                        "PNP0103" => "HPET",
                        "PNP0200" => "AT DMA controller",
                        "PNP0303" => "IBM Enhanced (101/102-key, PS/2 mouse support)",
                        "PNP030B" => "PS/2 keyboard",
                        "PNP0400" => "Standard LPT printer port",
                        "PNP0501" => "16550A-compatible COM port",
                        "PNP0A03" | "PNP0A08" => "PCI bus",
                        "PNP0A05" => "Generic ACPI bus",
                        "PNP0A06" => "Generic ACPI Extended-IO bus (EIO bus)",
                        "PNP0B00" => "AT real-time clock",
                        "PNP0C01" => "System board",
                        "PNP0C02" => "Reserved resources",
                        "PNP0C04" => "Math coprocessor",
                        "PNP0C09" => "Embedded controller",
                        "PNP0C0A" => "Battery",
                        "PNP0C0B" => "Fan",
                        "PNP0C0C" => "Power button",
                        "PNP0C0D" => "Lid sensor",
                        "PNP0C0E" => "Sleep button",
                        "PNP0C0F" => "PCI interrupt link",
                        "PNP0C50" => "I2C HID",
                        "PNP0F13" => "PS/2 port for PS/2-style mouse",
                        _ => "?",
                    };
                    log::debug!("{}: {} ({})", name, id, what);
                }
            }
        }
        Ok(())
    }
}
