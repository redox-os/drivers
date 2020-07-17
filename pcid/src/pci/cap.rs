use std::fmt;
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

    // function specific

    Sata    = 0x12, // only on AHCI functions
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum MsiCapability {
    _32BitAddress(MsiCapability32bAddr),
    _64BitAddress(MsiCapability64bAddr),
    _32BitAddressWithPvm(MsiCapability32bAddrWithPvm),
    _64BitAddressWithPvm(MsiCapability64bAddrWithPvm),
}
impl MsiCapability {
    pub fn kind(&self) -> MsiCapabilityKind {
        match self {
            Self::_32BitAddress(_) => MsiCapabilityKind::Addr32,
            Self::_64BitAddress(_) => MsiCapabilityKind::Addr64,
            Self::_32BitAddressWithPvm(_) => MsiCapabilityKind::Addr32Pvm,
            Self::_64BitAddressWithPvm(_) => MsiCapabilityKind::Addr64Pvm,
        }
    }
    pub fn construct(kind: MsiCapabilityKind, raw: MsiCapabilityRaw) -> Self {
        unsafe {
            match kind {
                MsiCapabilityKind::Addr32 => Self::_32BitAddress(raw.addr32),
                MsiCapabilityKind::Addr64 => Self::_64BitAddress(raw.addr64),
                MsiCapabilityKind::Addr32Pvm => Self::_32BitAddressWithPvm(raw.addr32pvm),
                MsiCapabilityKind::Addr64Pvm => Self::_64BitAddressWithPvm(raw.addr64pvm),
            }
        }
    }
}
#[repr(u8)]
pub enum MsiCapabilityKind {
    Addr32 = 0,
    Addr64,
    Addr32Pvm,
    Addr64Pvm,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union MsiCapabilityRaw {
    pub addr32: MsiCapability32bAddr,
    pub addr64: MsiCapability64bAddr,
    pub addr32pvm: MsiCapability32bAddrWithPvm,
    pub addr64pvm: MsiCapability64bAddrWithPvm,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MsiCapabilityRawTagged {
    pub kind: u8,
    pub raw: MsiCapabilityRaw,
}
impl MsiCapabilityRawTagged {
    pub fn kind(&self) -> Option<MsiCapabilityKind> {
        match self.kind {
            0 => Some(MsiCapabilityKind::Addr32),
            1 => Some(MsiCapabilityKind::Addr64),
            2 => Some(MsiCapabilityKind::Addr32Pvm),
            3 => Some(MsiCapabilityKind::Addr64Pvm),
            _ => None,
        }
    }
}
impl fmt::Debug for MsiCapabilityRawTagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(kind) = self.kind() {
            fmt::Debug::fmt(&MsiCapability::construct(kind, self.raw), f)
        } else {
            write!(f, "<MsiCapabilityRawTagged of unknown kind>")
        }
    }
}

pub trait MsiCapabilityProps {
    fn message_control(&self) -> u32;
    fn message_address(&self) -> u32;
    fn message_data(&self) -> u16;

    fn message_upper_address(&self) -> Option<u32> { None }
    fn mask_bits(&self) -> Option<u32> { None }
    fn pending_bits(&self) -> Option<u32> { None }
}
impl MsiCapabilityProps for MsiCapability32bAddr {
    fn message_address(&self) -> u32 {
        self.message_address
    }
    fn message_control(&self) -> u32 {
        self.message_control
    }
    fn message_data(&self) -> u16 {
        self.message_data
    }
}
impl MsiCapabilityProps for MsiCapability64bAddr {
    fn message_address(&self) -> u32 {
        self.message_address_lo
    }
    fn message_control(&self) -> u32 {
        self.message_control
    }
    fn message_data(&self) -> u16 {
        self.message_data
    }
    fn message_upper_address(&self) -> Option<u32> {
        Some(self.message_address_hi)
    }
}
impl MsiCapabilityProps for MsiCapability32bAddrWithPvm {
    fn message_address(&self) -> u32 {
        self.message_address
    }
    fn message_control(&self) -> u32 {
        self.message_control
    }
    fn message_data(&self) -> u16 {
        self.message_data as u16
    }
    fn mask_bits(&self) -> Option<u32> {
        Some(self.mask_bits)
    }
    fn pending_bits(&self) -> Option<u32> {
        Some(self.pending_bits)
    }
}
impl MsiCapabilityProps for MsiCapability64bAddrWithPvm {
    fn message_address(&self) -> u32 {
        self.message_address_lo
    }
    fn message_control(&self) -> u32 {
        self.message_control
    }
    fn message_data(&self) -> u16 {
        self.message_data as u16
    }
    fn message_upper_address(&self) -> Option<u32> {
        Some(self.message_address_hi)
    }
    fn mask_bits(&self) -> Option<u32> {
        Some(self.mask_bits)
    }
    fn pending_bits(&self) -> Option<u32> {
        Some(self.pending_bits)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsiCapability32bAddr {
    pub message_control: u32,
    pub message_address: u32,
    pub message_data: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsiCapability64bAddr {
    pub message_control: u32,
    pub message_address_lo: u32,
    pub message_address_hi: u32,
    pub message_data: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsiCapability32bAddrWithPvm {
    pub message_control: u32,
    pub message_address: u32,
    pub message_data: u32,
    pub mask_bits: u32,
    pub pending_bits: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MsiCapability64bAddrWithPvm {
    pub message_control: u32,
    pub message_address_lo: u32,
    pub message_address_hi: u32,
    pub message_data: u32,
    pub mask_bits: u32,
    pub pending_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(C)]
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
#[repr(C)]
pub struct PwrMgmtCapability {
    pub a: u32,
    pub b: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(C)]
pub struct MsixCapability {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Capability {
    Msi(MsiCapability),
    MsiX(MsixCapability),
    Pcie(PcieCapability),
    PwrMgmt(PwrMgmtCapability),
    FunctionSpecific(u8, Vec<u8>), // TODO: Arrayvec
    Other(u8),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapabilityRawTagged {
    pub id: u8,
    pub raw: CapabilityRaw,
}

#[derive(Clone, Copy)]
pub struct FunctionSpecific {
    pub id: u8,
    pub len: u16,
    pub bytes: [u8; 4096 - 0x40],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union CapabilityRaw {
    pub msi: MsiCapabilityRawTagged,
    pub msix: MsixCapability,
    pub pcie: PcieCapability,
    pub pwrmgmt: PwrMgmtCapability,

    // This may take up some space (that's the maximum size of the PCIe conf space), but since
    // capabilities aren't enumerated that often, it will not harm performance and/or memory usage
    // in the general case.
    //
    // Capabilities that are set often (like MSI), only use a few bytes of this struct for that,
    // even though the buffer will have to be a page large.
    pub func_specific: FunctionSpecific,
    pub other: (),
}

impl Capability {
    pub fn construct(id: u8, raw: CapabilityRaw) -> Option<Self> {
        unsafe {
            Some(if id == CapabilityId::Msi as u8 {
                Self::Msi(MsiCapability::construct(raw.msi.kind()?, raw.msi.raw))
            } else if id == CapabilityId::MsiX as u8 {
                Self::MsiX(raw.msix)
            } else if id == CapabilityId::Pcie as u8 {
                Self::Pcie(raw.pcie)
            } else if id == CapabilityId::PwrMgmt as u8 {
                Self::PwrMgmt(raw.pwrmgmt)
            } else if id == CapabilityId::Sata as u8 {
                Self::FunctionSpecific(id, raw.func_specific.bytes[..raw.func_specific.len as usize].to_vec())
            } else {
                Self::Other(id)
            })
        }
    }
    pub fn is_pcie(&self) -> bool {
        self.as_pcie().is_some()
    }
    pub fn as_pcie(&self) -> Option<&PcieCapability> {
        match self {
            &Self::Pcie(ref pcie) => Some(pcie),
            _ => None,
        }
    }
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
    unsafe fn parse_pwr<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::PwrMgmt(PwrMgmtCapability {
            a: reader.read_u32(u16::from(offset)),
            b: reader.read_u32(u16::from(offset + 4)),
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
        assert_eq!(offset & 0xF8, offset, "capability must be dword aligned");

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
        } else if capability_id == CapabilityId::Sata as u8 {
            Self::FunctionSpecific(capability_id, reader.read_range(offset.into(), 8))
        } else {
            log::warn!("unimplemented or malformed capability id: {}", capability_id);
            Self::Other(capability_id)
        }
    }
}

pub struct CapabilitiesIter<'a, R>(pub CapabilityOffsetsIter<'a, R>);

impl<'a, R> Iterator for CapabilitiesIter<'a, R>
where
    R: ConfigReader
{
    type Item = (u8, Capability);

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.0.next()?;
        Some((offset, unsafe { Capability::parse(self.0.reader, offset) }))
    }
}
