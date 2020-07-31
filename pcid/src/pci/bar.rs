use std::convert::TryFrom;
use std::fmt;

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::pci::func::ConfigReader;

type PciBars = ArrayVec<[PciBar; 6]>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum PciBar {
    MemorySpace32 { address: u32, prefetchable: bool },
    MemorySpace64 { address: u64, prefetchable: bool },
    IoSpace { address: u32 },
}

#[derive(Debug, Error)]
pub enum BarFromRawError {
    #[error("Either the BAR is memory space and 64 bit, and the input was 32 bit, or vice versa")]
    InvalidSize,

    #[error("BAR input has reserved bits with wrong values")]
    ReservedBits,
}

pub const BAR_INDICATOR_IOSPACE_SHIFT: u8 = 0;
pub const BAR_INDICATOR_IOSPACE_BIT: u32 = 1 << BAR_INDICATOR_IOSPACE_SHIFT;

pub const BAR_IOSPACE_BASE_ADDR_MASK: u32 = 0xFFFF_FFFC;
pub const BAR_IOSPACE_RSVD_SHIFT: u8 = 1;
pub const BAR_IOSPACE_RSVD_BIT: u32 = 1 << BAR_IOSPACE_RSVD_SHIFT;

pub const BAR_MEMSPACE_TY_MASK: u32 = 0x0000_0006;
pub const BAR_MEMSPACE_TY_SHIFT: u8 = 1;
pub const BAR_MEMSPACE_PREFETCH_SHIFT: u8 = 3;
pub const BAR_MEMSPACE_PREFETCH_BIT: u32 = 1 << BAR_MEMSPACE_PREFETCH_SHIFT;
pub const BAR_MEMSPACE32_ADDR_MASK: u32 = 0xFFFF_FFF0;
pub const BAR_MEMSPACE32_ADDR_SHIFT: u8 = 0;
pub const BAR_MEMSPACE64_ADDR_MASK: u64 = 0xFFFF_FFFF_FFFF_FFF0;
pub const BAR_MEMSPACE64_ADDR_SHIFT: u8 = 0;

#[repr(u8)]
pub enum MemoryBarType {
    Address32 = 0b00,
    // TODO: Handle the pre-PCI 3.0 encoding for addresses that are located within the first
    // megabyte, or something.
    Rsvd01 = 0b01,
    Address64 = 0b10,
    Rsvd11 = 0b11,
}
impl MemoryBarType {
    pub fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0b00 => Self::Address32,
            0b01 => Self::Rsvd01,
            0b10 => Self::Address64,
            0b11 => Self::Rsvd11,
            _ => return None,
        })
    }
}

impl PciBar {
    pub fn is_64_bit(&self) -> bool {
        matches!(self, PciBar::MemorySpace64 { .. })
    }
    fn parse_memspace(base: u32) -> Option<(bool, bool)> {
        let ty_raw = u8::try_from((base & BAR_MEMSPACE_TY_MASK) >> BAR_MEMSPACE_TY_SHIFT).unwrap();
        let ty = MemoryBarType::from_raw(ty_raw).unwrap();
        let prefetch = base & BAR_MEMSPACE_PREFETCH_BIT == BAR_MEMSPACE_PREFETCH_BIT;

        let is_64 = match ty {
            MemoryBarType::Address32 => false,
            MemoryBarType::Address64 => true,
            _rsvd => return None,
        };

        Some((prefetch, is_64))
    }
    pub fn from_raw_32(raw_u32: u32) -> Result<Option<Self>, BarFromRawError> {
        if raw_u32 == 0 {
            return Ok(None);
        }

        if raw_u32 & BAR_INDICATOR_IOSPACE_BIT == BAR_INDICATOR_IOSPACE_BIT {
            if raw_u32 & BAR_IOSPACE_RSVD_BIT == BAR_IOSPACE_RSVD_BIT {
                return Err(BarFromRawError::ReservedBits);
            }
            let address = raw_u32 & BAR_IOSPACE_BASE_ADDR_MASK;
            Ok(Some(Self::IoSpace { address }))
        } else {
            let (prefetchable, is_64) =
                Self::parse_memspace(raw_u32).ok_or(BarFromRawError::ReservedBits)?;
            if is_64 {
                return Err(BarFromRawError::InvalidSize);
            }
            let address = raw_u32 & BAR_MEMSPACE32_ADDR_MASK;
            Ok(Some(Self::MemorySpace32 {
                address,
                prefetchable,
            }))
        }
    }
    pub fn from_raw_64(raw_u64: u64) -> Result<Option<Self>, BarFromRawError> {
        if raw_u64 == 0 {
            return Ok(None);
        }

        let base = (raw_u64 & 0xFFFF_FFFF) as u32;
        if base & BAR_INDICATOR_IOSPACE_BIT == BAR_INDICATOR_IOSPACE_BIT {
            return Err(BarFromRawError::InvalidSize);
        }
        let (prefetchable, is_64) =
            Self::parse_memspace(base).ok_or(BarFromRawError::ReservedBits)?;
        if !is_64 {
            return Err(BarFromRawError::InvalidSize);
        }
        let address = raw_u64 & BAR_MEMSPACE64_ADDR_MASK;
        Ok(Some(Self::MemorySpace64 {
            address,
            prefetchable,
        }))
    }
    pub fn parse_00_header_bars(
        reader: &impl ConfigReader,
    ) -> Result<[Option<PciBar>; 6], BarFromRawError> {
        let range = (0x10..0x28).step_by(4);
        let mut bars = [None; 6];
        Self::parse_header_bars(reader, range, &mut bars)?;
        Ok(bars)
    }
    pub fn parse_01_header_bars(
        reader: &impl ConfigReader,
    ) -> Result<[Option<PciBar>; 2], BarFromRawError> {
        let range = (0x10..0x18).step_by(4);
        let mut bars = [None; 2];
        Self::parse_header_bars(reader, range, &mut bars)?;
        Ok(bars)
    }
    fn parse_header_bars(
        reader: &impl ConfigReader,
        mut range: impl Iterator<Item = u16>,
        bars: &mut [Option<PciBar>],
    ) -> Result<(), BarFromRawError> {
        let mut i = 0;

        while let Some(address) = range.next() {
            let dword_lo = unsafe { reader.read_u32(address) };

            match Self::from_raw_32(dword_lo) {
                Ok(bar) => bars[i] = bar,
                Err(BarFromRawError::InvalidSize) => {
                    let address_above = match range.next() {
                        Some(a) => a,

                        // This condition indicates that the last BAR (i.e. at address 0x24), was
                        // 64 bits, but there was no space for an upper 32 bits of that register.
                        None => return Err(BarFromRawError::InvalidSize),
                    };
                    let dword_hi = unsafe { reader.read_u32(address_above) };

                    let qword = (u64::from(dword_hi) << 32) | u64::from(dword_lo);
                    bars[i] = Self::from_raw_64(qword)?;

                    i += 1;
                }
                Err(BarFromRawError::ReservedBits) => return Err(BarFromRawError::ReservedBits),
            }
            i += 1;
        }
        Ok(())
    }
    pub fn address(&self) -> u64 {
        match *self {
            Self::MemorySpace32 { address, .. } | Self::IoSpace { address } => u64::from(address),
            Self::MemorySpace64 { address, .. } => address,
        }
    }
}

impl fmt::Display for PciBar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fn prefetchable_str(prefetchable: bool) -> &'static str {
            if prefetchable {
                "prefetchable"
            } else {
                "non-prefetchable"
            }
        }

        match *self {
            PciBar::MemorySpace32 {
                address,
                prefetchable,
            } => write!(
                f,
                "Memory at {:08x} (32-bit, {})",
                address,
                prefetchable_str(prefetchable)
            ),
            PciBar::MemorySpace64 {
                address,
                prefetchable,
            } => write!(
                f,
                "Memory at {:08x} (64-bit, {})",
                address,
                prefetchable_str(prefetchable)
            ),
            PciBar::IoSpace { address } => write!(f, "I/O ports at {:>04X}", address),
        }
    }
}
