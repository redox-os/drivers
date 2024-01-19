use std::convert::TryFrom;
use std::fmt;
use std::sync::{Mutex, Once};

use bit_field::BitField;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use syscall::io::{Io as _, Pio};

pub use self::bar::PciBar;
pub use self::bus::{PciBus, PciBusIter};
pub use self::class::PciClass;
pub use self::dev::{PciDev, PciDevIter};
pub use self::func::PciFunc;
pub use self::header::{PciHeader, PciHeaderError, PciHeaderType};

use log::info;

mod bar;
mod bus;
pub mod cap;
mod class;
mod dev;
pub mod func;
pub mod header;
pub mod msi;

pub trait CfgAccess {
    unsafe fn read(&self, addr: PciAddress, offset: u16) -> u32;
    unsafe fn write(&self, addr: PciAddress, offset: u16, value: u32);
}

// Copied from the pci_types crate, version 0.6.1.
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
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

pub struct Pci {
    lock: Mutex<()>,
    iopl_once: Once,
}

impl Pci {
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            iopl_once: Once::new(),
        }
    }

    pub fn buses<'pci>(&'pci self) -> PciIter<'pci> {
        PciIter::new(self)
    }

    fn set_iopl() {
        // make sure that pcid is not granted io port permission unless pcie memory-mapped
        // configuration space is not available.
        info!("PCI: couldn't find or access PCIe extended configuration, and thus falling back to PCI 3.0 io ports");
        unsafe {
            syscall::iopl(3).expect("pcid: failed to set iopl to 3");
        }
    }
    fn address(address: PciAddress, offset: u8) -> u32 {
        // TODO: Find the part of pcid that uses an unaligned offset!
        //
        // assert_eq!(offset & 0xFC, offset, "pci offset is not aligned");
        //
        let offset = offset & 0xFC;

        assert_eq!(
            address.segment(),
            0,
            "usage of multiple segments requires PCIe extended configuration"
        );

        0x80000000
            | (u32::from(address.bus()) << 16)
            | (u32::from(address.device()) << 11)
            | (u32::from(address.function()) << 8)
            | u32::from(offset)
    }
}
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl CfgAccess for Pci {
    unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
        let _guard = self.lock.lock().unwrap();

        self.iopl_once.call_once(Self::set_iopl);

        let offset =
            u8::try_from(offset).expect("offset too large for PCI 3.0 configuration space");
        let address = Self::address(address, offset);

        Pio::<u32>::new(0xCF8).write(address);
        Pio::<u32>::new(0xCFC).read()
    }

    unsafe fn write(&self, address: PciAddress, offset: u16, value: u32) {
        let _guard = self.lock.lock().unwrap();

        self.iopl_once.call_once(Self::set_iopl);

        let offset =
            u8::try_from(offset).expect("offset too large for PCI 3.0 configuration space");
        let address = Self::address(address, offset);

        Pio::<u32>::new(0xCF8).write(address);
        Pio::<u32>::new(0xCFC).write(value);
    }
}
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
impl CfgAccess for Pci {
    unsafe fn read(&self, addr: PciAddress, offset: u16) -> u32 {
        let _guard = self.lock.lock().unwrap();
        todo!("Pci::CfgAccess::read on this architecture")
    }

    unsafe fn write(&self, addr: PciAddress, offset: u16, value: u32) {
        let _guard = self.lock.lock().unwrap();
        todo!("Pci::CfgAccess::write on this architecture")
    }
}

pub struct PciIter<'pci> {
    pci: &'pci dyn CfgAccess,
    num: Option<u8>,
}

impl<'pci> PciIter<'pci> {
    pub fn new(pci: &'pci dyn CfgAccess) -> Self {
        PciIter { pci, num: Some(0) }
    }
}

impl<'pci> Iterator for PciIter<'pci> {
    type Item = PciBus<'pci>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.num {
            Some(bus_num) => {
                let bus = PciBus {
                    pci: self.pci,
                    num: bus_num,
                };
                self.num = bus_num.checked_add(1);
                Some(bus)
            }
            None => None,
        }
    }
}
