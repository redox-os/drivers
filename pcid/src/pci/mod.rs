use std::fmt;

use bit_field::BitField;
use serde::{Deserialize, Serialize};

pub use self::bar::PciBar;
pub use self::class::PciClass;
pub use self::func::PciFunc;
pub use self::id::FullDeviceId;

mod bar;
pub mod cap;
mod class;
pub mod func;
mod id;
pub mod msi;

pub trait CfgAccess {
    unsafe fn read(&self, addr: PciAddress, offset: u16) -> u32;
    unsafe fn write(&self, addr: PciAddress, offset: u16, value: u32);
}

// Copied from the pci_types crate, version 0.6.1. It has been modified to add serde support.
// FIXME If we start using it in the future use the upstream version instead.
/// The address of a PCIe function.
///
/// PCIe supports 65536 segments, each with 256 buses, each with 32 slots, each with 8 possible functions. We pack this into a `u32`:
///
/// ```ignore
/// 32                              16               8         3      0
///  +-------------------------------+---------------+---------+------+
///  |            segment            |      bus      | device  | func |
///  +-------------------------------+---------------+---------+------+
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct PciAddress(u32);

impl PciAddress {
    pub fn new(segment: u16, bus: u8, device: u8, function: u8) -> PciAddress {
        let mut result = 0;
        result.set_bits(0..3, function as u32);
        result.set_bits(3..8, device as u32);
        result.set_bits(8..16, bus as u32);
        result.set_bits(16..32, segment as u32);
        PciAddress(result)
    }

    pub fn segment(&self) -> u16 {
        self.0.get_bits(16..32) as u16
    }

    pub fn bus(&self) -> u8 {
        self.0.get_bits(8..16) as u8
    }

    pub fn device(&self) -> u8 {
        self.0.get_bits(3..8) as u8
    }

    pub fn function(&self) -> u8 {
        self.0.get_bits(0..3) as u8
    }
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}-{:02x}:{:02x}.{}",
            self.segment(),
            self.bus(),
            self.device(),
            self.function()
        )
    }
}

impl fmt::Debug for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}
