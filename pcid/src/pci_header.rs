use pci_types::{
    Bar as TyBar, ConfigRegionAccess, EndpointHeader, HeaderType, PciAddress,
    PciHeader as TyPciHeader, PciPciBridgeHeader,
};

use crate::pci::{FullDeviceId, PciBar};

#[derive(Debug, PartialEq)]
pub enum PciHeaderError {
    NoDevice,
    UnknownHeaderType(HeaderType),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharedPciHeader {
    full_device_id: FullDeviceId,
    addr: PciAddress,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PciEndpointHeader {
    shared: SharedPciHeader,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PciHeader {
    General(PciEndpointHeader),
    PciToPci {
        shared: SharedPciHeader,
        secondary_bus_num: u8,
    },
}

impl PciHeader {
    /// Parse the bytes found in the Configuration Space of the PCI device into
    /// a more usable PciHeader.
    pub fn from_reader(
        access: &impl ConfigRegionAccess,
        addr: PciAddress,
    ) -> Result<PciHeader, PciHeaderError> {
        let header = TyPciHeader::new(addr);
        let (vendor_id, device_id) = header.id(access);

        if vendor_id == 0xffff && device_id == 0xffff {
            return Err(PciHeaderError::NoDevice);
        }

        let (revision, class, subclass, interface) = header.revision_and_class(access);
        let header_type = header.header_type(access);
        let shared = SharedPciHeader {
            full_device_id: FullDeviceId {
                vendor_id,
                device_id,
                class,
                subclass,
                interface,
                revision,
            },
            addr,
        };

        match header_type {
            HeaderType::Endpoint => Ok(PciHeader::General(PciEndpointHeader { shared })),
            HeaderType::PciPciBridge => {
                let bridge_header = PciPciBridgeHeader::from_header(header, access).unwrap();
                let secondary_bus_num = bridge_header.secondary_bus_number(access);
                Ok(PciHeader::PciToPci {
                    shared,
                    secondary_bus_num,
                })
            }
            ty => Err(PciHeaderError::UnknownHeaderType(ty)),
        }
    }

    /// Return all identifying information of the PCI function.
    pub fn full_device_id(&self) -> &FullDeviceId {
        match self {
            PciHeader::General(PciEndpointHeader {
                shared:
                    SharedPciHeader {
                        full_device_id: device_id,
                        ..
                    },
                ..
            })
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
}

impl PciEndpointHeader {
    pub fn address(&self) -> PciAddress {
        self.shared.addr
    }

    pub fn endpoint_header(&self, access: &impl ConfigRegionAccess) -> EndpointHeader {
        EndpointHeader::from_header(TyPciHeader::new(self.shared.addr), access).unwrap()
    }

    pub fn full_device_id(&self) -> &FullDeviceId {
        &self.shared.full_device_id
    }

    /// Return the Headers BARs.
    pub fn bars(&self, access: &impl ConfigRegionAccess) -> [PciBar; 6] {
        let endpoint_header = self.endpoint_header(access);

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
}

#[cfg(test)]
mod test {
    use std::convert::TryInto;

    use pci_types::device_type::DeviceType;
    use pci_types::{ConfigRegionAccess, PciAddress};

    use super::{PciHeader, PciHeaderError};

    struct TestCfgAccess<'a> {
        addr: PciAddress,
        bytes: &'a [u8],
    }

    impl ConfigRegionAccess for TestCfgAccess<'_> {
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
        match header {
            PciHeader::General { .. } => {}
            _ => panic!("wrong header type"),
        }
        assert_eq!(header.full_device_id().device_id, 0x1533);
        assert_eq!(header.full_device_id().vendor_id, 0x8086);
        assert_eq!(header.full_device_id().revision, 3);
        assert_eq!(header.full_device_id().interface, 0);
        assert_eq!(
            DeviceType::from((
                header.full_device_id().class,
                header.full_device_id().subclass
            )),
            DeviceType::EthernetController
        );
        assert_eq!(header.full_device_id().subclass, 0);
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
