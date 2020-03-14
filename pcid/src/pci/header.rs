use byteorder::{LittleEndian, ByteOrder};

use super::func::ConfigReader;
use super::class::PciClass;
use super::bar::PciBar;
use bitflags::bitflags;

#[derive(Debug, PartialEq)]
pub enum PciHeaderError {
    NoDevice,
    UnknownHeaderType(u8)
}

bitflags! {
    /// Flags found in the status register of a PCI device
    pub struct PciHeaderType: u8 {
        /// A general PCI device (Type 0x01).
        const GENERAL       = 0b00000000;
        /// A PCI-to-PCI bridge device (Type 0x01).
        const PCITOPCI      = 0b00000001;
        /// A PCI-to-PCI bridge device (Type 0x02).
        const CARDBUSBRIDGE = 0b00000010;
        /// A multifunction device.
        const MULTIFUNCTION = 0b01000000;
        /// Mask used for fetching the header type.
        const HEADER_TYPE   = 0b00000011;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PciHeader {
    General {
        vendor_id: u16,
        device_id: u16,
        command: u16,
        status: u16,
        revision: u8,
        interface: u8,
        subclass: u8,
        class: PciClass,
        cache_line_size: u8,
        latency_timer: u8,
        header_type: PciHeaderType,
        bist: u8,
        bars: [PciBar; 6],
        cardbus_cis_ptr: u32,
        subsystem_vendor_id: u16,
        subsystem_id: u16,
        expansion_rom_bar: u32,
        cap_pointer: u8,
        interrupt_line: u8,
        interrupt_pin: u8,
        min_grant: u8,
        max_latency: u8
    },
    PciToPci {
        vendor_id: u16,
        device_id: u16,
        command: u16,
        status: u16,
        revision: u8,
        interface: u8,
        subclass: u8,
        class: PciClass,
        cache_line_size: u8,
        latency_timer: u8,
        header_type: PciHeaderType,
        bist: u8,
        bars: [PciBar; 2],
        primary_bus_num: u8,
        secondary_bus_num: u8,
        subordinate_bus_num: u8,
        secondary_latency_timer: u8,
        io_base: u8,
        io_limit: u8,
        secondary_status: u16,
        mem_base: u16,
        mem_limit: u16,
        prefetch_base: u16,
        prefetch_limit: u16,
        prefetch_base_upper: u32,
        prefetch_limit_upper: u32,
        io_base_upper: u16,
        io_limit_upper: u16,
        cap_pointer: u8,
        expansion_rom: u32,
        interrupt_line: u8,
        interrupt_pin : u8,
        bridge_control: u16
    }
}

impl PciHeader {
    /// Parse the bytes found in the Configuration Space of the PCI device into
    /// a more usable PciHeader.
    pub fn from_reader<T: ConfigReader>(reader: T) -> Result<PciHeader, PciHeaderError> {
        if unsafe { reader.read_u32(0) } != 0xffffffff {
            // Read the initial 16 bytes and set variables used by all header types.
            let bytes = unsafe { reader.read_range(0, 16) };
            let vendor_id = LittleEndian::read_u16(&bytes[0..2]);
            let device_id = LittleEndian::read_u16(&bytes[2..4]);
            let command = LittleEndian::read_u16(&bytes[4..6]);
            let status = LittleEndian::read_u16(&bytes[6..8]);
            let revision = bytes[8];
            let interface = bytes[9];
            let subclass = bytes[10];
            let class = PciClass::from(bytes[11]);
            let cache_line_size = bytes[12];
            let latency_timer = bytes[13];
            let header_type = PciHeaderType::from_bits_truncate(bytes[14]);
            let bist = bytes[15];
            match header_type & PciHeaderType::HEADER_TYPE {
                PciHeaderType::GENERAL => {
                    let bytes = unsafe { reader.read_range(16, 48) };
                    let bars = [
                        PciBar::from(LittleEndian::read_u32(&bytes[0..4])),
                        PciBar::from(LittleEndian::read_u32(&bytes[4..8])),
                        PciBar::from(LittleEndian::read_u32(&bytes[8..12])),
                        PciBar::from(LittleEndian::read_u32(&bytes[12..16])),
                        PciBar::from(LittleEndian::read_u32(&bytes[16..20])),
                        PciBar::from(LittleEndian::read_u32(&bytes[20..24])),
                    ];
                    let cardbus_cis_ptr = LittleEndian::read_u32(&bytes[24..28]);
                    let subsystem_vendor_id = LittleEndian::read_u16(&bytes[28..30]);
                    let subsystem_id = LittleEndian::read_u16(&bytes[30..32]);
                    let expansion_rom_bar = LittleEndian::read_u32(&bytes[32..36]);
                    let cap_pointer = bytes[36];
                    let interrupt_line = bytes[44];
                    let interrupt_pin = bytes[45];
                    let min_grant = bytes[46];
                    let max_latency = bytes[47];
                    Ok(PciHeader::General {
                        vendor_id, device_id, command, status, revision, interface,
                        subclass, class, cache_line_size, latency_timer, header_type,
                        bist, bars, cardbus_cis_ptr, subsystem_vendor_id, subsystem_id,
                        expansion_rom_bar, cap_pointer, interrupt_line, interrupt_pin,
                        min_grant, max_latency
                    })
                },
                PciHeaderType::PCITOPCI => {
                    let bytes = unsafe { reader.read_range(16, 48) };
                    let bars = [
                        PciBar::from(LittleEndian::read_u32(&bytes[0..4])),
                        PciBar::from(LittleEndian::read_u32(&bytes[4..8])),
                    ];
                    let primary_bus_num = bytes[8];
                    let secondary_bus_num = bytes[9];
                    let subordinate_bus_num = bytes[10];
                    let secondary_latency_timer = bytes[11];
                    let io_base = bytes[12];
                    let io_limit = bytes[13];
                    let secondary_status = LittleEndian::read_u16(&bytes[14..16]);
                    let mem_base = LittleEndian::read_u16(&bytes[16..18]);
                    let mem_limit = LittleEndian::read_u16(&bytes[18..20]);
                    let prefetch_base = LittleEndian::read_u16(&bytes[20..22]);
                    let prefetch_limit = LittleEndian::read_u16(&bytes[22..24]);
                    let prefetch_base_upper = LittleEndian::read_u32(&bytes[24..28]);
                    let prefetch_limit_upper = LittleEndian::read_u32(&bytes[28..32]);
                    let io_base_upper = LittleEndian::read_u16(&bytes[32..34]);
                    let io_limit_upper = LittleEndian::read_u16(&bytes[34..36]);
                    let cap_pointer = bytes[36];
                    let expansion_rom = LittleEndian::read_u32(&bytes[40..44]);
                    let interrupt_line = bytes[44];
                    let interrupt_pin = bytes[45];
                    let bridge_control = LittleEndian::read_u16(&bytes[46..48]);
                    Ok(PciHeader::PciToPci {
                        vendor_id, device_id, command, status, revision, interface,
                        subclass, class, cache_line_size, latency_timer, header_type,
                        bist, bars, primary_bus_num, secondary_bus_num, subordinate_bus_num,
                        secondary_latency_timer, io_base, io_limit, secondary_status,
                        mem_base, mem_limit, prefetch_base, prefetch_limit, prefetch_base_upper,
                        prefetch_limit_upper, io_base_upper, io_limit_upper, cap_pointer,
                        expansion_rom, interrupt_line, interrupt_pin, bridge_control
                    })

                },
                id => Err(PciHeaderError::UnknownHeaderType(id.bits()))
            }
        } else {
            Err(PciHeaderError::NoDevice)
        }
    }

    /// Return the Header Type.
    pub fn header_type(&self) -> PciHeaderType {
        match self {
            &PciHeader::General { header_type, .. } | &PciHeader::PciToPci { header_type, .. } => header_type,
        }
    }

    /// Return the Vendor ID field.
    pub fn vendor_id(&self) -> u16 {
        match self {
            &PciHeader::General { vendor_id, .. } | &PciHeader::PciToPci { vendor_id, .. } => vendor_id,
        }
    }

    /// Return the Device ID field.
    pub fn device_id(&self) -> u16 {
        match self {
            &PciHeader::General { device_id, .. } | &PciHeader::PciToPci { device_id, .. } => device_id,
        }
    }

    /// Return the Revision field.
    pub fn revision(&self) -> u8 {
        match self {
            &PciHeader::General { revision, .. } | &PciHeader::PciToPci { revision, .. } => revision,
        }
    }

    /// Return the Interface field.
    pub fn interface(&self) -> u8 {
        match self {
            &PciHeader::General { interface, .. } | &PciHeader::PciToPci { interface, .. } => interface,
        }
    }

    /// Return the Subclass field.
    pub fn subclass(&self) -> u8 {
        match self {
            &PciHeader::General { subclass, .. } | &PciHeader::PciToPci { subclass, .. } => subclass,
        }
    }

    /// Return the Class field.
    pub fn class(&self) -> PciClass {
        match self {
            &PciHeader::General { class, .. } | &PciHeader::PciToPci { class, .. } => class,
        }
    }

    /// Return the Headers BARs.
    pub fn bars(&self) -> &[PciBar] {
        match self {
            &PciHeader::General { ref bars, .. } => bars,
            &PciHeader::PciToPci { ref bars, .. } => bars,
        }
    }

    /// Return the BAR at the given index.
    ///
    /// # Panics
    /// This function panics if the requested BAR index is beyond the length of the header
    /// types BAR array.
    pub fn get_bar(&self, idx: usize) -> PciBar {
        match self {
            &PciHeader::General { bars, .. } => {
                assert!(idx < 6, "the general PCI device only has 6 BARs");
                bars[idx]
            },
            &PciHeader::PciToPci { bars, .. } => {
                assert!(idx < 2, "the general PCI device only has 2 BARs");
                bars[idx]
            }
        }
    }

    /// Return the Interrupt Line field.
    pub fn interrupt_line(&self) -> u8 {
        match self {
            &PciHeader::General { interrupt_line, .. } | &PciHeader::PciToPci { interrupt_line, .. } =>
                interrupt_line,
        }
    }

    pub fn cap_pointer(&self) -> u8 {
        match self {
            &PciHeader::General { cap_pointer, .. } | &PciHeader::PciToPci { cap_pointer, .. } => cap_pointer,
        }
    }
}

#[cfg(test)]
impl<'a> ConfigReader for &'a [u8] {
    unsafe fn read_u32(&self, offset: u8) -> u32 {
        let offset = offset as usize;
        assert!(offset < self.len());
        LittleEndian::read_u32(&self[offset..offset + 4])
    }
}

#[cfg(test)]
mod test {
    use super::{PciHeaderError, PciHeader, PciHeaderType};
    use super::super::func::ConfigReader;
    use super::super::class::PciClass;
    use super::super::bar::PciBar;

    const IGB_DEV_BYTES: [u8; 256] = [
        0x86, 0x80, 0x33, 0x15, 0x07, 0x04, 0x10, 0x00, 0x03, 0x00, 0x00, 0x02, 0x10, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x50, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x01, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x58, 0xf7,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd9, 0x15, 0x33, 0x15,
        0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x01, 0x00, 0x00,
        0x01, 0x50, 0x23, 0xc8, 0x08, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x05, 0x70, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x11, 0xa0, 0x04, 0x80, 0x03, 0x00, 0x00, 0x00, 0x03, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        0x10, 0x00, 0x02, 0x00, 0xc2, 0x8c, 0x00, 0x10, 0x0f, 0x28, 0x19, 0x00, 0x11, 0x5c, 0x42, 0x00,
        0x42, 0x00, 0x11, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    ];

    #[test]
    fn tset_parse_igb_dev() {
        let header = PciHeader::from_reader(&IGB_DEV_BYTES[..]).unwrap();
        assert_eq!(header.header_type(), PciHeaderType::GENERAL);
        assert_eq!(header.device_id(), 0x1533);
        assert_eq!(header.vendor_id(), 0x8086);
        assert_eq!(header.revision(), 3);
        assert_eq!(header.interface(), 0);
        assert_eq!(header.class(), PciClass::Network);
        assert_eq!(header.subclass(), 0);
        assert_eq!(header.bars().len(), 6);
        assert_eq!(header.get_bar(0), PciBar::Memory(0xf7500000));
        assert_eq!(header.get_bar(1), PciBar::None);
        assert_eq!(header.get_bar(2), PciBar::Port(0xb000));
        assert_eq!(header.get_bar(3), PciBar::Memory(0xf7580000));
        assert_eq!(header.get_bar(4), PciBar::None);
        assert_eq!(header.get_bar(5), PciBar::None);
        assert_eq!(header.interrupt_line(), 10);
    }

    #[test]
    fn test_parse_nonexistent() {
        let bytes = [
            0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff
        ];
        assert_eq!(PciHeader::from_reader(&bytes[..]), Err(PciHeaderError::NoDevice));
    }

    #[test]
    fn test_read_range() {
        let res = unsafe { (&IGB_DEV_BYTES[..]).read_range(0, 4) };
        assert_eq!(res, &[0x86, 0x80, 0x33, 0x15][..]);

        let res = unsafe { (&IGB_DEV_BYTES[..]).read_range(16, 32) };
        let expected = [
            0x00, 0x00, 0x50, 0xf7, 0x00, 0x00, 0x00, 0x00,
            0x01, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x58, 0xf7,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xd9, 0x15, 0x33, 0x15
        ];
        assert_eq!(res, expected);
    }

    macro_rules! read_range_should_panic {
        ($name:ident, $len:expr) => {
            #[test]
            #[should_panic(expected = "assertion failed: len > 3 && len % 4 == 0")]
            fn $name() {
                let _ = unsafe { (&IGB_DEV_BYTES[..]).read_range(0, $len) };
            }
        }
    }

    read_range_should_panic!(test_short_len, 2);
    read_range_should_panic!(test_not_mod_4_len, 7);
}
