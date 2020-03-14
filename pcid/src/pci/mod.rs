use std::sync::Mutex;

pub use self::bar::PciBar;
pub use self::bus::{PciBus, PciBusIter};
pub use self::class::PciClass;
pub use self::dev::{PciDev, PciDevIter};
pub use self::func::PciFunc;
pub use self::header::{PciHeader, PciHeaderError, PciHeaderType};

mod bar;
mod bus;
pub mod cap;
mod class;
mod dev;
mod func;
pub mod header;

pub struct Pci {
    lock: Mutex<()>,
}

impl Pci {
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
        }
    }

    pub fn buses<'pci>(&'pci self) -> PciIter<'pci> {
        PciIter::new(self)
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub unsafe fn read_nolock(&self, bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
        let address = 0x80000000 | ((bus as u32) << 16) | ((dev as u32) << 11) | ((func as u32) << 8) | ((offset as u32) & 0xFC);
        let value: u32;
        asm!("mov dx, 0xCF8
              out dx, eax
              mov dx, 0xCFC
              in eax, dx"
             : "={eax}"(value) : "{eax}"(address) : "dx" : "intel", "volatile");
        value
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub unsafe fn read(&self, bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
        let _guard = self.lock.lock().unwrap();
        self.read_nolock(bus, dev, func, offset)
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub unsafe fn write_nolock(&self, bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
        let address = 0x80000000 | ((bus as u32) << 16) | ((dev as u32) << 11) | ((func as u32) << 8) | ((offset as u32) & 0xFC);
        asm!("mov dx, 0xCF8
              out dx, eax"
             : : "{eax}"(address) : "dx" : "intel", "volatile");
        asm!("mov dx, 0xCFC
              out dx, eax"
             : : "{eax}"(value) : "dx" : "intel", "volatile");
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub unsafe fn write(&self, bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
        let _guard = self.lock.lock().unwrap();
        self.write_nolock(bus, dev, func, offset, value)
    }
}

pub struct PciIter<'pci> {
    pci: &'pci Pci,
    num: u32
}

impl<'pci> PciIter<'pci> {
    pub fn new(pci: &'pci Pci) -> Self {
        PciIter {
            pci: pci,
            num: 0
        }
    }
}

impl<'pci> Iterator for PciIter<'pci> {
    type Item = PciBus<'pci>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num < 255 { /* TODO: Do not ignore 0xFF bus */
            let bus = PciBus {
                pci: self.pci,
                num: self.num as u8
            };
            self.num += 1;
            Some(bus)
        } else {
            None
        }
    }
}
