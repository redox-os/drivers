use pci_types::{
    ConfigRegionAccess, EndpointHeader, HeaderType, PciAddress, PciHeader as TyPciHeader,
    PciPciBridgeHeader,
};

use crate::pci::FullDeviceId;

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
}
