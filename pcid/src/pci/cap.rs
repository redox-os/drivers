use super::func::PciFunc;

use serde::{Serialize, Deserialize};

pub struct CapabilitiesIter<'a> {
    offset: u8,
    func: &'a PciFunc<'a>,
}
impl<'a> CapabilitiesIter<'a> {
    pub fn new(offset: u8, func: &'a PciFunc) -> Self {
        Self {
            offset,
            func,
        }
    }
}
impl<'a> Iterator for CapabilitiesIter<'a> {
    type Item = Capability;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = unsafe {
            // mask RsvdP bits
            self.offset = self.offset & 0xFC;

            if self.offset == 0 { return None };

            let first_dword = self.func.read_u32(u16::from(self.offset));
            let next = ((first_dword >> 8) & 0xFF) as u8;

            let offset = self.offset;
            self.offset = next;

            offset
        };

        let cap = unsafe {
            Capability::parse(self.func, offset)
        };

        Some(cap)
    }
}

#[repr(u8)]
pub enum CapabilityId {
    PwrMgmt = 0x01,
    Msi     = 0x05,
    MsiX    = 0x11,
    Pcie    = 0x10,
    Vendor  = 0x09,

    // function specific
    Sata    = 0x12, // only on AHCI functions
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum MsiCapability {
    _32BitAddress {
        cap_offset: u8,
        message_control: u32,
        message_address: u32,
        message_data: u16,
    },
    _64BitAddress {
        cap_offset: u8,
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u16,
    },
    _32BitAddressWithPvm {
        cap_offset: u8,
        message_control: u32,
        message_address: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
    _64BitAddressWithPvm {
        cap_offset: u8,
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsixCapability {
    pub cap_offset: u8,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct VendorSpecificCapability {
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Capability {
    Msi(MsiCapability),
    MsiX(MsixCapability),
    Vendor(VendorSpecificCapability),
    Other(u8),
}

impl Capability {
    pub fn as_msi(&self) -> Option<&MsiCapability> {
        match self {
            &Self::Msi(ref msi) => Some(msi),
            _ => None,
        }
    }
    pub fn as_msix(&self) -> Option<&MsixCapability> {
        match self {
            &Self::MsiX(ref msix) => Some(msix),
            _ => None,
        }
    }
    pub fn as_msi_mut(&mut self) -> Option<&mut MsiCapability> {
        match self {
            &mut Self::Msi(ref mut msi) => Some(msi),
            _ => None,
        }
    }
    pub fn as_msix_mut(&mut self) -> Option<&mut MsixCapability> {
        match self {
            &mut Self::MsiX(ref mut msix) => Some(msix),
            _ => None,
        }
    }
    unsafe fn parse_msi(func: &PciFunc, offset: u8) -> Self {
        Self::Msi(MsiCapability::parse(func, offset))
    }
    unsafe fn parse_msix(func: &PciFunc, offset: u8) -> Self {
        Self::MsiX(MsixCapability {
            cap_offset: offset,
            a: func.read_u32(u16::from(offset)),
            b: func.read_u32(u16::from(offset + 4)),
            c: func.read_u32(u16::from(offset + 8)),
        })
    }
    unsafe fn parse_vendor(func: &PciFunc, offset: u8) -> Self {
        let next = func.read_u8(u16::from(offset+1));
        let length = func.read_u8(u16::from(offset+2));
        log::info!("Vendor specific offset: {offset:#02x} next: {next:#02x} cap len: {length:#02x}");
        let data = if length > 0 {
            let mut raw_data = func.read_range(offset.into(), length.into());
            raw_data.drain(3..).collect()
        } else {
            log::warn!("Vendor specific capability is invalid");
            Vec::new()
        };
        Self::Vendor(VendorSpecificCapability {
            data
        })
    }
    unsafe fn parse(func: &PciFunc, offset: u8) -> Self {
        assert_eq!(offset & 0xFC, offset, "capability must be dword aligned");

        let dword = func.read_u32(u16::from(offset));
        let capability_id = (dword & 0xFF) as u8;

        if capability_id == CapabilityId::Msi as u8 {
            Self::parse_msi(func, offset)
        } else if capability_id == CapabilityId::MsiX as u8 {
            Self::parse_msix(func, offset)
        } else if capability_id == CapabilityId::Vendor as u8 {
            Self::parse_vendor(func, offset)
        } else {
            if capability_id != CapabilityId::Pcie as u8
                && capability_id != CapabilityId::PwrMgmt as u8
                && capability_id != CapabilityId::Sata as u8
            {
                log::warn!(
                    "unimplemented or malformed capability id: {}",
                    capability_id
                );
            }
            Self::Other(capability_id)
        }
    }
}
