use std::convert::TryInto;

use super::bar::{BarFromRawError, PciBar};
use super::class::PciClass;
use super::func::ConfigReader;

use crate::driver_interface::LegacyInterruptPin;

#[derive(Debug, PartialEq)]
pub enum PciHeaderError {
    NoDevice,
    UnknownHeaderType(u8),

    // TODO: Hardware often sucks, so find some good way to actually workaround badly defined bars
    // (such as memory bars with 0b01 or 0b11 types, or a 64-bit bar in the last dword where it
    // won't fit).
    InvalidBars,
}

/// Flags found in the status register of a PCI device
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PciHeaderLayout {
    /// A general PCI device (Type 0x00).
    General = 0x00,
    /// A PCI-to-PCI bridge device (Type 0x01).
    PciToPci = 0x01,
    /// A PCI-to-PCI bridge device (Type 0x02).
    CardbusBridge = 0x02,
}
impl Default for PciHeaderLayout {
    fn default() -> Self {
        Self::General
    }
}
impl PciHeaderLayout {
    pub fn from_raw(raw: u8) -> Option<Self> {
        assert_eq!(raw & PciHeaderType::HEADER_LAYOUT_MASK, raw);

        Some(match raw {
            0x00 => Self::General,
            0x01 => Self::PciToPci,
            0x02 => Self::CardbusBridge,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PciHeaderType {
    pub layout: PciHeaderLayout,
    pub multifunction: bool,
}
impl PciHeaderType {
    /// A multifunction device.
    pub const MUTLIFUNCTION: u8 = 0x80;

    /// Mask used for fetching the header type.
    pub const HEADER_LAYOUT_MASK: u8 = 0x7F;

    pub fn from_raw(raw: u8) -> Option<Self> {
        let layout = PciHeaderLayout::from_raw(raw & Self::HEADER_LAYOUT_MASK)?;
        let multifunction = raw & Self::MUTLIFUNCTION == Self::MUTLIFUNCTION;

        Some(Self {
            layout,
            multifunction,
        })
    }
}

// TODO: Use repr(packed) to maybe speed up memory-mapped configuration, or to make things easier
// when parsing PCIe-only capabilities. Right now it's not that convenient to use though, since the
// "&raw x" RFC isn't ready yet.

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct PciHeaderGeneral {
    pub header_base: PciHeaderBase,
    pub bars: [u32; 6],
    pub cardbus_cis_ptr: u32,
    pub subsystem_vendor_id: u16,
    pub subsystem_id: u16,
    pub expansion_rom_bar: u32,
    pub cap_pointer: u8,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub min_grant: u8,
    pub max_latency: u8,
}
unsafe impl plain::Plain for PciHeaderGeneral {}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct PciHeaderBase {
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub revision: u8,
    pub interface: u8,
    pub subclass: u8,
    pub class: PciClass,
    pub cache_line_size: u8,
    pub latency_timer: u8,
    pub header_type: PciHeaderType,
    pub bist: u8,
}
unsafe impl plain::Plain for PciHeaderBase {}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct PciHeaderBridge {
    pub header_base: PciHeaderBase,
    pub bars: [u32; 2],
    pub primary_bus_num: u8,
    pub secondary_bus_num: u8,
    pub subordinate_bus_num: u8,
    pub secondary_latency_timer: u8,
    pub io_base: u8,
    pub io_limit: u8,
    pub secondary_status: u16,
    pub mem_base: u16,
    pub mem_limit: u16,
    pub prefetch_base: u16,
    pub prefetch_limit: u16,
    pub prefetch_base_upper: u32,
    pub prefetch_limit_upper: u32,
    pub io_base_upper: u16,
    pub io_limit_upper: u16,
    pub cap_pointer: u8,
    pub expansion_rom: u32,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub bridge_control: u16,
}
unsafe impl plain::Plain for PciHeaderBridge {}

fn interrupt_pin_from_raw(raw: u8) -> Option<LegacyInterruptPin> {
    match raw {
        0 => None,
        1 => Some(LegacyInterruptPin::IntA),
        2 => Some(LegacyInterruptPin::IntB),
        3 => Some(LegacyInterruptPin::IntC),
        4 => Some(LegacyInterruptPin::IntD),

        other => {
            log::warn!("pcid: invalid interrupt pin: {}", other);
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PciHeader {
    General(PciHeaderGeneral),
    PciToPci(PciHeaderBridge),
}

impl PciHeader {
    /// Parse the bytes found in the Configuration Space of the PCI device into
    /// a more usable PciHeader.
    pub fn from_reader<T: ConfigReader>(reader: T) -> Result<PciHeader, PciHeaderError> {
        let vendor_and_dev_id = unsafe { reader.read_u32(0x00) };
        let vendor_id = (vendor_and_dev_id & 0xFFFF) as u16;
        let device_id = ((vendor_and_dev_id >> 16) & 0xFFFF) as u16;

        if vendor_id == 0xFFFF {
            return Err(PciHeaderError::NoDevice);
        }

        let cmd_and_sts = unsafe { reader.read_u32(0x04) };
        let base_misc1 = u32::to_le_bytes(unsafe { reader.read_u32(0x08) });
        let base_misc2 = unsafe { reader.read_u32(0x0C) };

        let header_type_raw = ((base_misc2) >> 16) as u8;

        let base = PciHeaderBase {
            // 00h
            vendor_id,
            device_id,

            // 04h
            command: (cmd_and_sts & 0xFFFF) as u16,
            status: ((cmd_and_sts >> 16) & 0xFFFF) as u16,

            // 08h
            revision: base_misc1[0],
            interface: base_misc1[1],
            subclass: base_misc1[2],
            class: PciClass::from(base_misc1[3]),

            // 0Ch
            cache_line_size: (base_misc2 & 0xFF) as u8,
            latency_timer: ((base_misc2 >> 8) & 0xFF) as u8,
            header_type: PciHeaderType::from_raw(header_type_raw)
                .ok_or(PciHeaderError::UnknownHeaderType(header_type_raw))?,
            bist: ((base_misc2 >> 24) & 0xFF) as u8,
        };

        match base.header_type.layout {
            PciHeaderLayout::General => unsafe {
                let subsystem_dword = reader.read_u32(0x2C);
                let int_dword = reader.read_u32(0x3C);

                Ok(PciHeader::General(PciHeaderGeneral {
                    // 00h
                    header_base: base,
                    // 10h
                    bars: [
                        reader.read_u32(0x10),
                        reader.read_u32(0x14),
                        reader.read_u32(0x18),
                        reader.read_u32(0x1C),
                        reader.read_u32(0x20),
                        reader.read_u32(0x24),
                    ],
                    // 28h
                    cardbus_cis_ptr: reader.read_u32(0x28),
                    // 2Ch
                    subsystem_vendor_id: (subsystem_dword & 0xFFFF) as u16,
                    subsystem_id: ((subsystem_dword >> 16) & 0xFFFF) as u16,
                    // 30h
                    expansion_rom_bar: reader.read_u32(0x30),
                    // 34h
                    cap_pointer: (reader.read_u32(0x34) & 0xFF) as u8,
                    // 3Ch
                    interrupt_line: (int_dword & 0xFF) as u8,
                    interrupt_pin: ((int_dword >> 8) & 0xFF) as u8,
                    min_grant: ((int_dword >> 16) & 0xFF) as u8,
                    max_latency: ((int_dword >> 24) & 0xFF) as u8,
                }))
            },
            PciHeaderLayout::PciToPci => {
                let mut bytes = [0u8; 48];
                unsafe { reader.read_range_into(16, &mut bytes) };

                fn read_u16(slice: &[u8]) -> u16 {
                    u16::from_le_bytes(slice.try_into().unwrap())
                }
                fn read_u32(slice: &[u8]) -> u32 {
                    u32::from_le_bytes(slice.try_into().unwrap())
                }

                let bars = [
                    read_u32(bytes[..4].try_into().unwrap()),
                    read_u32(bytes[4..8].try_into().unwrap()),
                ];

                let primary_bus_num = bytes[8];
                let secondary_bus_num = bytes[9];
                let subordinate_bus_num = bytes[10];
                let secondary_latency_timer = bytes[11];
                let io_base = bytes[12];
                let io_limit = bytes[13];
                let secondary_status = read_u16(&bytes[14..16]);
                let mem_base = read_u16(&bytes[16..18]);
                let mem_limit = read_u16(&bytes[18..20]);
                let prefetch_base = read_u16(&bytes[20..22]);
                let prefetch_limit = read_u16(&bytes[22..24]);
                let prefetch_base_upper = read_u32(&bytes[24..28]);
                let prefetch_limit_upper = read_u32(&bytes[28..32]);
                let io_base_upper = read_u16(&bytes[32..34]);
                let io_limit_upper = read_u16(&bytes[34..36]);
                let cap_pointer = bytes[36];
                let expansion_rom = read_u32(&bytes[40..44]);
                let interrupt_line = bytes[44];
                let interrupt_pin = bytes[45];
                let bridge_control = read_u16(&bytes[46..48]);
                Ok(PciHeader::PciToPci(PciHeaderBridge {
                    header_base: base,
                    bars,
                    primary_bus_num,
                    secondary_bus_num,
                    subordinate_bus_num,
                    secondary_latency_timer,
                    io_base,
                    io_limit,
                    secondary_status,
                    mem_base,
                    mem_limit,
                    prefetch_base,
                    prefetch_limit,
                    prefetch_base_upper,
                    prefetch_limit_upper,
                    io_base_upper,
                    io_limit_upper,
                    cap_pointer,
                    expansion_rom,
                    interrupt_line,
                    interrupt_pin,
                    bridge_control,
                }))
            }
            // TODO: While I don't think anyone has a machine with card bus support, it might be
            // fun for completeness.
            PciHeaderLayout::CardbusBridge => {
                Err(PciHeaderError::UnknownHeaderType(header_type_raw))
            }
        }
    }

    pub fn base(&self) -> &PciHeaderBase {
        match *self {
            Self::General(PciHeaderGeneral {
                ref header_base, ..
            }) => header_base,
            Self::PciToPci(PciHeaderBridge {
                ref header_base, ..
            }) => header_base,
        }
    }

    /// Return the Header's Base Address Registers.
    pub fn bars(&self) -> Result<[Option<PciBar>; 6], BarFromRawError> {
        match *self {
            PciHeader::General(PciHeaderGeneral { bars, .. }) => PciBar::parse_00_header_bars(bars),
            PciHeader::PciToPci(PciHeaderBridge { bars, .. }) => {
                let mut all_bars = [None; 6];
                let bars = PciBar::parse_01_header_bars(bars)?;
                all_bars[..2].copy_from_slice(&bars);
                Ok(all_bars)
            }
        }
    }
    pub fn bar(&self, index: usize) -> Result<Option<PciBar>, BarFromRawError> {
        Ok(self.bars()?[index])
    }

    /// Returns the "Interrupt Line", which the device doesn't use, but that is still used by the
    /// 8259 PIC or I/O APIC for INTx# interrupts.
    pub fn legacy_interrupt_line(&self) -> u8 {
        match *self {
            PciHeader::General(PciHeaderGeneral { interrupt_line, .. })
            | PciHeader::PciToPci(PciHeaderBridge { interrupt_line, .. }) => interrupt_line,
        }
    }
    pub fn legacy_interrupt_pin(&self) -> Option<LegacyInterruptPin> {
        let raw_pin = match *self {
            PciHeader::General(PciHeaderGeneral { interrupt_pin, .. })
            | PciHeader::PciToPci(PciHeaderBridge { interrupt_pin, .. }) => interrupt_pin,
        };
        match raw_pin {
            0 => None,
            1 => Some(LegacyInterruptPin::IntA),
            2 => Some(LegacyInterruptPin::IntB),
            3 => Some(LegacyInterruptPin::IntC),
            4 => Some(LegacyInterruptPin::IntD),
            _ => None,
        }
    }

    /// Returns the offset within the configuration space used by this header. This is undefined
    /// and should not be used (the capabilities list will simply be treated as empty), unless the
    /// "Capabilities List" bit is set in the device status.
    pub fn cap_pointer(&self) -> u8 {
        match *self {
            PciHeader::General(PciHeaderGeneral { cap_pointer, .. }) => cap_pointer,
            PciHeader::PciToPci(PciHeaderBridge { cap_pointer, .. }) => cap_pointer,
        }
    }
}
impl AsRef<PciHeaderBase> for PciHeader {
    fn as_ref(&self) -> &PciHeaderBase {
        self.base()
    }
}

#[cfg(test)]
impl<'a> ConfigReader for &'a [u8] {
    unsafe fn read_u32(&self, offset: u16) -> u32 {
        let offset = offset as usize;
        assert!(offset < self.len());
        u32::from_le_bytes(self[offset..offset + 4].try_into().unwrap())
    }
}

#[cfg(test)]
mod test {
    use super::super::bar::PciBar;
    use super::super::class::PciClass;
    use super::super::func::ConfigReader;
    use super::LegacyInterruptPin;
    use super::{PciHeader, PciHeaderError, PciHeaderLayout};

    const IGB_DEV_BYTES: [u8; 256] = [
        0x86, 0x80, 0x33, 0x15, 0x07, 0x04, 0x10, 0x00, 0x03, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x50, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x01, 0xb0, 0x00, 0x00, 0x00, 0x00,
        0x58, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd9,
        0x15, 0x33, 0x15, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0a, 0x01, 0x00, 0x00, 0x01, 0x50, 0x23, 0xc8, 0x08, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x70, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0xa0, 0x04, 0x80, 0x03, 0x00, 0x00, 0x00,
        0x03, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x10, 0x00, 0x02, 0x00, 0xc2,
        0x8c, 0x00, 0x10, 0x0f, 0x28, 0x19, 0x00, 0x11, 0x5c, 0x42, 0x00, 0x42, 0x00, 0x11, 0x10,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    #[test]
    fn parse_igb_dev() {
        let header = PciHeader::from_reader(&IGB_DEV_BYTES[..]).unwrap();
        let base = header.base();
        assert_eq!(base.header_type.layout, PciHeaderLayout::General);
        assert_eq!(base.device_id, 0x1533);
        assert_eq!(base.vendor_id, 0x8086);
        assert_eq!(base.revision, 3);
        assert_eq!(base.interface, 0);
        assert_eq!(base.class, PciClass::Network);
        assert_eq!(base.subclass, 0);
        assert_eq!(header.bars().unwrap().len(), 6);
        assert_eq!(
            header.bar(0).unwrap(),
            Some(PciBar::MemorySpace32 {
                address: 0xf7500000,
                prefetchable: false
            })
        );
        assert_eq!(header.bar(1).unwrap(), None);
        assert_eq!(header.bar(2).unwrap(), Some(PciBar::IoSpace { address: 0xb000 }));
        assert_eq!(
            header.bar(3).unwrap(),
            Some(PciBar::MemorySpace32 {
                address: 0xf7580000,
                prefetchable: false
            })
        );
        assert_eq!(header.bar(4).unwrap(), None);
        assert_eq!(header.bar(5).unwrap(), None);
        assert_eq!(header.legacy_interrupt_line(), 10);
        assert_eq!(
            header.legacy_interrupt_pin(),
            Some(LegacyInterruptPin::IntA)
        );
    }

    #[test]
    fn parse_nonexistent() {
        let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            PciHeader::from_reader(&bytes[..]),
            Err(PciHeaderError::NoDevice)
        );
    }

    #[test]
    fn read_range() {
        let res = unsafe { (&IGB_DEV_BYTES[..]).read_range(0, 4) };
        assert_eq!(res, &[0x86, 0x80, 0x33, 0x15][..]);

        let res = unsafe { (&IGB_DEV_BYTES[..]).read_range(16, 32) };
        let expected = [
            0x00, 0x00, 0x50, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x01, 0xb0, 0x00, 0x00, 0x00, 0x00,
            0x58, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xd9, 0x15, 0x33, 0x15,
        ];
        assert_eq!(res, expected);
    }

    #[test]
    #[should_panic(expected = "assertion failed: buf.len() >= 4")]
    fn short_len() {
        let _ = unsafe { (&IGB_DEV_BYTES[..]).read_range(0, 2) };
    }
    #[test]
    #[should_panic(expected = "assertion failed: buf.len() % 4 == 0")]
    fn not_mod_4_len() {
        let _ = unsafe { (&IGB_DEV_BYTES[..]).read_range(0, 7) };
    }
}
