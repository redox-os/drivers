use std::convert::TryInto;

use bitflags::bitflags;
use byteorder::{ByteOrder, LittleEndian};
use pci_types::{
    Bar as TyBar, ConfigRegionAccess, EndpointHeader, PciAddress, PciHeader as TyPciHeader,
};
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharedPciHeader {
    full_device_id: FullDeviceId,
    command: u16,
    status: u16,
    header_type: PciHeaderType,
    addr: PciAddress,
}

// FIXME move out of pcid_interface
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PciHeader {
    General {
        shared: SharedPciHeader,
        subsystem_vendor_id: u16,
        subsystem_id: u16,
        cap_pointer: u8,
        interrupt_line: u8,
        interrupt_pin: u8,
    },
    PciToPci {
        shared: SharedPciHeader,
        secondary_bus_num: u8,
        cap_pointer: u8,
        interrupt_line: u8,
        interrupt_pin: u8,
        bridge_control: u16,
    },
}

impl PciHeader {
    /// Parse the bytes found in the Configuration Space of the PCI device into
    /// a more usable PciHeader.
    pub fn from_reader(
        access: &impl ConfigRegionAccess,
        addr: PciAddress,
    ) -> Result<PciHeader, PciHeaderError> {
        if unsafe { access.read(addr, 0) } == 0xffffffff {
            return Err(PciHeaderError::NoDevice);
        }

        let header = TyPciHeader::new(addr);
        let (vendor_id, device_id) = header.id(access);
        let command_and_status = unsafe { access.read(addr, 4) };
        let command = (command_and_status & 0xffff) as u16;
        let status = (command_and_status >> 16) as u16;
        let (revision, class, subclass, interface) = header.revision_and_class(access);
        let header_type = PciHeaderType::from_bits_truncate(
            ((unsafe { access.read(addr, 12) } >> 24) & 0xff) as u8,
        );
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
            addr,
        };

        match header_type & PciHeaderType::HEADER_TYPE {
            PciHeaderType::GENERAL => {
                let endpoint_header = EndpointHeader::from_header(header, access).unwrap();
                let bytes = unsafe {
                    let mut ret = Vec::with_capacity(48);
                    for offset in (16..64).step_by(4) {
                        ret.extend(access.read(addr, offset).to_le_bytes());
                    }
                    ret
                };
                let (subsystem_id, subsystem_vendor_id) = endpoint_header.subsystem(access);
                let cap_pointer = bytes[36];
                let (interrupt_pin, interrupt_line) = endpoint_header.interrupt(access);
                Ok(PciHeader::General {
                    shared,
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
                        ret.extend(access.read(addr, offset).to_le_bytes());
                    }
                    ret
                };
                let secondary_bus_num = bytes[9];
                let cap_pointer = bytes[36];
                let interrupt_line = bytes[44];
                let interrupt_pin = bytes[45];
                let bridge_control = LittleEndian::read_u16(&bytes[46..48]);
                Ok(PciHeader::PciToPci {
                    shared,
                    secondary_bus_num,
                    cap_pointer,
                    interrupt_line,
                    interrupt_pin,
                    bridge_control,
                })
            }
            id => Err(PciHeaderError::UnknownHeaderType(id.bits())),
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
    // FIXME use pci_types::Bar instead
    pub fn bars(&self, access: &impl ConfigRegionAccess) -> [PciBar; 6] {
        let endpoint_header = match *self {
            PciHeader::General {
                shared: SharedPciHeader { addr, .. },
                ..
            } => EndpointHeader::from_header(TyPciHeader::new(addr), access).unwrap(),
            PciHeader::PciToPci { .. } => unreachable!(),
        };

        let mut bars = [PciBar::None; 6];
        let mut skip = false;
        for i in 0..6 {
            if skip {
                skip = false;
                continue;
            }
            match endpoint_header.bar(i, access) {
                Some(TyBar::Io { port }) => {
                    bars[i as usize] = PciBar::Port(port.try_into().unwrap())
                }
                Some(TyBar::Memory32 {
                    address,
                    size,
                    prefetchable: _,
                }) => {
                    bars[i as usize] = PciBar::Memory32 {
                        addr: address,
                        size,
                    }
                }
                Some(TyBar::Memory64 {
                    address,
                    size,
                    prefetchable: _,
                }) => {
                    bars[i as usize] = PciBar::Memory64 {
                        addr: address,
                        size,
                    };
                    skip = true; // Each 64bit memory BAR occupies two slots
                }
                None => bars[i as usize] = PciBar::None,
            }
        }
        bars
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
    use crate::pci::PciClass;

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
