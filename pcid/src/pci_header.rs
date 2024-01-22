use bitflags::bitflags;
use byteorder::{ByteOrder, LittleEndian};
use pci_types::{ConfigRegionAccess, PciAddress};
use serde::{Deserialize, Serialize};

use crate::pci::{FullDeviceId, PciBar, PciClass};

#[derive(Debug, PartialEq)]
pub enum PciHeaderError {
    NoDevice,
    UnknownHeaderType(u8),
}

bitflags! {
    /// Flags found in the status register of a PCI device
    #[derive(Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedPciHeader {
    full_device_id: FullDeviceId,
    command: u16,
    status: u16,
    header_type: PciHeaderType,
}

// FIXME move out of pcid_interface
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PciHeader {
    General {
        shared: SharedPciHeader,
        bars: [PciBar; 6],
        subsystem_vendor_id: u16,
        subsystem_id: u16,
        cap_pointer: u8,
        interrupt_line: u8,
        interrupt_pin: u8,
    },
    PciToPci {
        shared: SharedPciHeader,
        bars: [PciBar; 2],
        secondary_bus_num: u8,
        cap_pointer: u8,
        interrupt_line: u8,
        interrupt_pin: u8,
        bridge_control: u16,
    },
}

impl PciHeader {
    fn get_bars(bytes: &[u8], bars: &mut [PciBar]) {
        let mut i = 0;
        while i < bars.len() {
            let offset = i * 4;
            let bar_bytes = match bytes.get(offset..offset + 4) {
                Some(some) => some,
                None => continue,
            };

            match PciBar::from(LittleEndian::read_u32(bar_bytes)) {
                PciBar::Memory64(mut addr) => {
                    let high_bytes = match bytes.get(offset + 4..offset + 8) {
                        Some(some) => some,
                        None => continue,
                    };
                    addr |= (LittleEndian::read_u32(high_bytes) as u64) << 32;
                    bars[i] = PciBar::Memory64(addr);
                    i += 2;
                }
                bar => {
                    bars[i] = bar;
                    i += 1;
                }
            }
        }
    }

    /// Parse the bytes found in the Configuration Space of the PCI device into
    /// a more usable PciHeader.
    pub fn from_reader(
        cfg_access: &dyn ConfigRegionAccess,
        addr: PciAddress,
    ) -> Result<PciHeader, PciHeaderError> {
        if unsafe { cfg_access.read(addr, 0) } != 0xffffffff {
            // Read the initial 16 bytes and set variables used by all header types.
            let bytes = unsafe {
                let mut ret = Vec::with_capacity(16);
                for offset in (0..16).step_by(4) {
                    ret.extend(cfg_access.read(addr, offset).to_le_bytes());
                }
                ret
            };
            let vendor_id = LittleEndian::read_u16(&bytes[0..2]);
            let device_id = LittleEndian::read_u16(&bytes[2..4]);
            let command = LittleEndian::read_u16(&bytes[4..6]);
            let status = LittleEndian::read_u16(&bytes[6..8]);
            let revision = bytes[8];
            let interface = bytes[9];
            let subclass = bytes[10];
            let class = bytes[11];
            let header_type = PciHeaderType::from_bits_truncate(bytes[14]);
            let shared = SharedPciHeader {
                full_device_id: FullDeviceId {
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    interface,
                    revision,
                },
                command,
                status,
                header_type,
            };

            match header_type & PciHeaderType::HEADER_TYPE {
                PciHeaderType::GENERAL => {
                    let bytes = unsafe {
                        let mut ret = Vec::with_capacity(48);
                        for offset in (16..64).step_by(4) {
                            ret.extend(cfg_access.read(addr, offset).to_le_bytes());
                        }
                        ret
                    };
                    let mut bars = [PciBar::None; 6];
                    Self::get_bars(&bytes, &mut bars);
                    let subsystem_vendor_id = LittleEndian::read_u16(&bytes[28..30]);
                    let subsystem_id = LittleEndian::read_u16(&bytes[30..32]);
                    let cap_pointer = bytes[36];
                    let interrupt_line = bytes[44];
                    let interrupt_pin = bytes[45];
                    Ok(PciHeader::General {
                        shared,
                        bars,
                        subsystem_vendor_id,
                        subsystem_id,
                        cap_pointer,
                        interrupt_line,
                        interrupt_pin,
                    })
                }
                PciHeaderType::PCITOPCI => {
                    let bytes = unsafe {
                        let mut ret = Vec::with_capacity(48);
                        for offset in (16..64).step_by(4) {
                            ret.extend(cfg_access.read(addr, offset).to_le_bytes());
                        }
                        ret
                    };
                    let mut bars = [PciBar::None; 2];
                    Self::get_bars(&bytes, &mut bars);
                    let secondary_bus_num = bytes[9];
                    let cap_pointer = bytes[36];
                    let interrupt_line = bytes[44];
                    let interrupt_pin = bytes[45];
                    let bridge_control = LittleEndian::read_u16(&bytes[46..48]);
                    Ok(PciHeader::PciToPci {
                        shared,
                        bars,
                        secondary_bus_num,
                        cap_pointer,
                        interrupt_line,
                        interrupt_pin,
                        bridge_control,
                    })
                }
                id => Err(PciHeaderError::UnknownHeaderType(id.bits())),
            }
        } else {
            Err(PciHeaderError::NoDevice)
        }
    }

    /// Return the Header Type.
    pub fn header_type(&self) -> PciHeaderType {
        match self {
            &PciHeader::General {
                shared: SharedPciHeader { header_type, .. },
                ..
            }
            | &PciHeader::PciToPci {
                shared: SharedPciHeader { header_type, .. },
                ..
            } => header_type,
        }
    }

    /// Return all identifying information of the PCI function.
    pub fn full_device_id(&self) -> &FullDeviceId {
        match self {
            PciHeader::General {
                shared:
                    SharedPciHeader {
                        full_device_id: device_id,
                        ..
                    },
                ..
            }
            | PciHeader::PciToPci {
                shared:
                    SharedPciHeader {
                        full_device_id: device_id,
                        ..
                    },
                ..
            } => device_id,
        }
    }

    /// Return the Vendor ID field.
    pub fn vendor_id(&self) -> u16 {
        self.full_device_id().vendor_id
    }

    /// Return the Device ID field.
    pub fn device_id(&self) -> u16 {
        self.full_device_id().device_id
    }

    /// Return the Revision field.
    pub fn revision(&self) -> u8 {
        self.full_device_id().revision
    }

    /// Return the Interface field.
    pub fn interface(&self) -> u8 {
        self.full_device_id().interface
    }

    /// Return the Subclass field.
    pub fn subclass(&self) -> u8 {
        self.full_device_id().subclass
    }

    /// Return the Class field.
    pub fn class(&self) -> PciClass {
        PciClass::from(self.full_device_id().class)
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
            }
            &PciHeader::PciToPci { bars, .. } => {
                assert!(idx < 2, "the general PCI device only has 2 BARs");
                bars[idx]
            }
        }
    }

    /// Return the Interrupt Line field.
    pub fn interrupt_line(&self) -> u8 {
        match self {
            &PciHeader::General { interrupt_line, .. }
            | &PciHeader::PciToPci { interrupt_line, .. } => interrupt_line,
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            &PciHeader::General {
                shared: SharedPciHeader { status, .. },
                ..
            }
            | &PciHeader::PciToPci {
                shared: SharedPciHeader { status, .. },
                ..
            } => status,
        }
    }

    pub fn cap_pointer(&self) -> u8 {
        match self {
            &PciHeader::General { cap_pointer, .. } | &PciHeader::PciToPci { cap_pointer, .. } => {
                cap_pointer
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::convert::TryInto;

    use pci_types::{ConfigRegionAccess, PciAddress};

    use super::{PciHeader, PciHeaderError, PciHeaderType};
    use crate::pci::{PciBar, PciClass};

    struct TestCfgAccess<'a> {
        addr: PciAddress,
        bytes: &'a [u8],
    }

    impl ConfigRegionAccess for TestCfgAccess<'_> {
        fn function_exists(&self, _address: PciAddress) -> bool {
            unreachable!();
        }

        unsafe fn read(&self, addr: PciAddress, offset: u16) -> u32 {
            assert_eq!(addr, self.addr);
            let offset = offset as usize;
            assert!(offset < self.bytes.len());
            u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
        }

        unsafe fn write(&self, _addr: PciAddress, _offset: u16, _value: u32) {
            unreachable!("should not write during tests");
        }
    }

    #[rustfmt::skip]
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
        let header = PciHeader::from_reader(
            &TestCfgAccess {
                addr: PciAddress::new(0, 2, 4, 0),
                bytes: &IGB_DEV_BYTES,
            },
            PciAddress::new(0, 2, 4, 0),
        )
        .unwrap();
        assert_eq!(header.header_type(), PciHeaderType::GENERAL);
        assert_eq!(header.device_id(), 0x1533);
        assert_eq!(header.vendor_id(), 0x8086);
        assert_eq!(header.revision(), 3);
        assert_eq!(header.interface(), 0);
        assert_eq!(header.class(), PciClass::Network);
        assert_eq!(header.subclass(), 0);
        assert_eq!(header.bars().len(), 6);
        assert_eq!(header.get_bar(0), PciBar::Memory32(0xf7500000));
        assert_eq!(header.get_bar(1), PciBar::None);
        assert_eq!(header.get_bar(2), PciBar::Port(0xb000));
        assert_eq!(header.get_bar(3), PciBar::Memory32(0xf7580000));
        assert_eq!(header.get_bar(4), PciBar::None);
        assert_eq!(header.get_bar(5), PciBar::None);
        assert_eq!(header.interrupt_line(), 10);
    }

    #[test]
    fn test_parse_nonexistent() {
        let bytes = &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            PciHeader::from_reader(
                &TestCfgAccess {
                    addr: PciAddress::new(0, 2, 4, 0),
                    bytes,
                },
                PciAddress::new(0, 2, 4, 0),
            ),
            Err(PciHeaderError::NoDevice)
        );
    }
}
