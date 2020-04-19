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
            assert_eq!(self.offset & 0xF8, self.offset, "capability must be dword aligned");

            if self.offset == 0 { return None };

            let first_dword = dbg!(self.reader.read_u32(dbg!(u16::from(self.offset))));
            let next = ((first_dword >> 8) & 0xFF) as u8;

            let offset = self.offset;
            self.offset = next;

            Some(offset)
        }
    }
}

#[repr(u8)]
pub enum CapabilityId {
    Msi = 0x05,
    MsiX = 0x11,
    Pcie = 0x10,
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


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PcieCapability {
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsixCapability {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Capability {
    Msi(MsiCapability),
    MsiX(MsixCapability),
    Pcie(PcieCapability),
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
    pub fn into_msi(self) -> Option<MsiCapability> {
        match self {
            Self::Msi(msi) => Some(msi),
            _ => None,
        }
    }
    pub fn into_msix(self) -> Option<MsixCapability> {
        match self {
            Self::MsiX(msix) => Some(msix),
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
    unsafe fn parse_pcie<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        // TODO
        Self::Pcie(PcieCapability {})
    }
    unsafe fn parse<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        assert_eq!(offset & 0xF8, offset, "capability must be dword aligned");

        let dword = reader.read_u32(u16::from(offset));
        let capability_id = (dword & 0xFF) as u8;

        if capability_id == CapabilityId::Msi as u8 {
            Self::parse_msi(reader, offset)
        } else if capability_id == CapabilityId::MsiX as u8 {
            Self::parse_msix(reader, offset)
        } else if capability_id == CapabilityId::Pcie as u8 {
            Self::parse_pcie(reader, offset)
        } else {
            Self::Other(capability_id)
            //panic!("unimplemented or malformed capability id: {}", capability_id)
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
