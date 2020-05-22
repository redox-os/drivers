use std::mem;

use syscall::{Io, Mmio};
use serde::{Serialize, Deserialize};

use crate::pci::func::ConfigReader;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub kind: CapabilityKind,
    pub cap_version: u8,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum CapabilityKind {
    // TODO: AER
    Unknown(u16),
    Aer(AerCap),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(packed)]
pub struct AerCap {
    pub header: u32,
    pub uncorr_error_sts: u32,
    pub uncorr_error_mask: u32,
    pub uncorr_error_severity: u32,
    pub corr_error_sts: u32,
    pub corr_error_mask: u32,
    pub aer_cap_ctl: u32,
    pub header_log: [u32; 4],
    pub root_err_cmd: u32,
    pub root_err_sts: u32,
    pub err_id_regs: u32,
    pub tlp_prefix_log: [u32; 4],
}
unsafe impl plain::Plain for AerCap {}

#[repr(u16)]
pub enum CapabilityId {
    Aer = 0x0001,
}

impl CapabilityKind {
    unsafe fn parse_aer<R: ConfigReader>(reader: &R, offset: u16) -> AerCap {
        let mut cap = None;
        reader.with_mapped_mem(&mut |slice| {
            let slice = slice.unwrap();
            assert_eq!((slice.as_ptr() as usize) % mem::align_of::<AerCap>(), 0);
            assert_eq!(offset & 0b11, offset);
            assert!(slice.len() * mem::size_of::<u32>() >= mem::size_of::<AerCap>());
            cap = Some(std::ptr::read_volatile(slice.as_ptr() as *mut AerCap as *const AerCap));
        });
        cap.expect("closure not called")
    }
}

impl Capability {
    unsafe fn parse<R: ConfigReader>(reader: &R, offset: u16) -> Self {
        let dword = reader.read_u32(offset);
        let cap_version = ((dword & 0x000F_0000) >> 16) as u8;
        let cap_id = (dword & 0x0000_FFFF) as u16;

        let kind = if cap_id == CapabilityId::Aer as u16 {
            CapabilityKind::Aer(CapabilityKind::parse_aer(reader, offset))
        } else {
            log::warn!("Unimplemented/malformed PCIe capability id: {} (full dword {}) at offset {:#0x}", cap_id, dword, offset);
            CapabilityKind::Unknown(cap_id)
        };
        Capability {
            kind,
            cap_version,
        }
    }
}

pub struct CapabilityOffsetsIter<'a, R> {
    pointer: u16,
    reader: &'a R,
}
impl<'a, R> CapabilityOffsetsIter<'a, R> {
    pub unsafe fn new(root_ptr: u16, reader: &'a R) -> Self {
        Self {
            pointer: root_ptr,
            reader,
        }
    }
}
impl<'a, R> Iterator for CapabilityOffsetsIter<'a, R>
where
    R: ConfigReader
{
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        let current_pointer = self.pointer;

        if self.pointer == 0 {
            return None;
        }
        let dword = unsafe { self.reader.read_u32(self.pointer) };

        if dword == 0 { return None }

        let capability_id = (dword & 0x0000_FFFF) as u16;

        let next_offset_raw = ((dword & 0xFFF0_0000) >> 20) as u16;
        let next_offset = next_offset_raw & 0xFFC; // mask off the bottom 2 RsvdP bits.

        // Technically only allowed in the Root Complex Register Block.
        if capability_id == 0xFFFF && next_offset == 0x000 {
            return None;
        }

        // Technically only allowed in the PCIe configuration space.
        /*if next_offset <= 0x0FF {
            log::warn!("PCI extended capability header had a next capability offset outside the extended region ({} <= 0FFh)", next_offset);
            return None;
        }*/

        self.pointer = next_offset;
        Some(current_pointer)
    }
}

pub struct CapabilitiesIter<'a, R>(pub CapabilityOffsetsIter<'a, R>);

impl<'a, R> Iterator for CapabilitiesIter<'a, R>
where
    R: ConfigReader
{
    type Item = (u16, Capability);

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.0.next()?;
        Some((offset, unsafe { Capability::parse(self.0.reader, offset) }))
    }
}
