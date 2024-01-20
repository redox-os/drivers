use super::func::ConfigReader;
use serde::{Serialize, Deserialize};

pub struct CapabilityOffsetsIter<'a, R> {
    offset: u8,
    reader: &'a R,
}
impl<'a, R> CapabilityOffsetsIter<'a, R> {
    pub fn new(offset: u8, reader: &'a R) -> Self {
        Self {
            offset,
            reader,
        }
    }
}
impl<'a, R> Iterator for CapabilityOffsetsIter<'a, R>
where
    R: ConfigReader
{
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            // mask RsvdP bits
            self.offset = self.offset & 0xFC;

            if self.offset == 0 { return None };

            let first_dword = self.reader.read_u32(u16::from(self.offset));
            let next = ((first_dword >> 8) & 0xFF) as u8;

            let offset = self.offset;
            self.offset = next;

            Some(offset)
        }
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
        message_control: u32,
        message_address: u32,
        message_data: u16,
    },
    _64BitAddress {
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u16,
    },
    _32BitAddressWithPvm {
        message_control: u32,
        message_address: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
    _64BitAddressWithPvm {
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
}


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PcieCapability {
    pub pcie_caps: u32,
    pub dev_caps: u32,
    pub dev_sts_ctl: u32,
    pub link_caps: u32,
    pub link_sts_ctl: u32,
    pub slot_caps: u32,
    pub slot_sts_ctl: u32,
    pub root_cap_ctl: u32,
    pub root_sts: u32,
    pub dev_caps2: u32,
    pub dev_sts_ctl2: u32,
    pub link_caps2: u32,
    pub link_sts_ctl2: u32,
    pub slot_caps2: u32,
    pub slot_sts_ctl2: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PwrMgmtCapability {
    pub a: u32,
    pub b: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsixCapability {
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
    Pcie(PcieCapability),
    PwrMgmt(PwrMgmtCapability),
    Vendor(VendorSpecificCapability),
    FunctionSpecific(u8, Vec<u8>), // TODO: Arrayvec
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
    unsafe fn parse_msi<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::Msi(MsiCapability::parse(reader, offset))
    }
    unsafe fn parse_msix<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::MsiX(MsixCapability {
            a: reader.read_u32(u16::from(offset)),
            b: reader.read_u32(u16::from(offset + 4)),
            c: reader.read_u32(u16::from(offset + 8)),
        })
    }
    unsafe fn parse_pwr<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::PwrMgmt(PwrMgmtCapability {
            a: reader.read_u32(u16::from(offset)),
            b: reader.read_u32(u16::from(offset + 4)),
        })
    }
    unsafe fn parse_vendor<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        let next = reader.read_u8(u16::from(offset+1));
        let length = reader.read_u8(u16::from(offset+2));
        log::info!("Vendor specific offset: {offset:#02x} next: {next:#02x} cap len: {length:#02x}");
        let data = if length > 0 {
            let mut raw_data = reader.read_range(offset.into(), length.into());
            raw_data.drain(3..).collect()
        } else {
            log::warn!("Vendor specific capability is invalid");
            Vec::new()
        };
        Self::Vendor(VendorSpecificCapability {
            data
        })
    }
    unsafe fn parse_pcie<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        let offset = u16::from(offset);

        Self::Pcie(PcieCapability {
            pcie_caps:      reader.read_u32(offset),
            dev_caps:       reader.read_u32(offset + 0x04),
            dev_sts_ctl:    reader.read_u32(offset + 0x08),
            link_caps:      reader.read_u32(offset + 0x0C),
            link_sts_ctl:   reader.read_u32(offset + 0x10),
            slot_caps:      reader.read_u32(offset + 0x14),
            slot_sts_ctl:   reader.read_u32(offset + 0x18),
            root_cap_ctl:   reader.read_u32(offset + 0x1C),
            root_sts:       reader.read_u32(offset + 0x20),
            dev_caps2:      reader.read_u32(offset + 0x24),
            dev_sts_ctl2:   reader.read_u32(offset + 0x28),
            link_caps2:     reader.read_u32(offset + 0x2C),
            link_sts_ctl2:  reader.read_u32(offset + 0x30),
            slot_caps2:     reader.read_u32(offset + 0x34),
            slot_sts_ctl2:  reader.read_u32(offset + 0x38),
        })
    }
    unsafe fn parse<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        assert_eq!(offset & 0xFC, offset, "capability must be dword aligned");

        let dword = reader.read_u32(u16::from(offset));
        let capability_id = (dword & 0xFF) as u8;

        if capability_id == CapabilityId::Msi as u8 {
            Self::parse_msi(reader, offset)
        } else if capability_id == CapabilityId::MsiX as u8 {
            Self::parse_msix(reader, offset)
        } else if capability_id == CapabilityId::Pcie as u8 {
            Self::parse_pcie(reader, offset)
        } else if capability_id == CapabilityId::PwrMgmt as u8{
            Self::parse_pwr(reader, offset)
        } else if capability_id == CapabilityId::Vendor as u8 {
            Self::parse_vendor(reader, offset)
        } else if capability_id == CapabilityId::Sata as u8 {
            Self::FunctionSpecific(capability_id, reader.read_range(offset.into(), 8))
        } else {
            log::warn!("unimplemented or malformed capability id: {}", capability_id);
            Self::Other(capability_id)
        }
    }
}

pub struct CapabilitiesIter<'a, R> {
    pub inner: CapabilityOffsetsIter<'a, R>,
}

impl<'a, R> Iterator for CapabilitiesIter<'a, R>
where
    R: ConfigReader
{
    type Item = (u8, Capability);

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.inner.next()?;
        Some((offset, unsafe { Capability::parse(self.inner.reader, offset) }))
    }
}
