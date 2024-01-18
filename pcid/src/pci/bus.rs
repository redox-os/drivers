use super::{CfgAccess, PciDev};

pub struct PciBus<'pci> {
    pub pci: &'pci dyn CfgAccess,
    pub num: u8,
}

impl<'pci> PciBus<'pci> {
    pub fn devs(&'pci self) -> PciBusIter<'pci> {
        PciBusIter::new(self)
    }
}

pub struct PciBusIter<'pci> {
    bus: &'pci PciBus<'pci>,
    num: u8,
}

impl<'pci> PciBusIter<'pci> {
    pub fn new(bus: &'pci PciBus<'pci>) -> Self {
        PciBusIter { bus, num: 0 }
    }
}

impl<'pci> Iterator for PciBusIter<'pci> {
    type Item = PciDev<'pci>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.num {
            dev_num if dev_num < 32 => {
                let dev = PciDev {
                    bus: self.bus,
                    num: self.num,
                };
                self.num += 1;
                Some(dev)
            }
            _ => None,
        }
    }
}
