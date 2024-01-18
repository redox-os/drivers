use super::{CfgAccess, PciAddress, PciBus, PciFunc};

#[derive(Copy, Clone)]
pub struct PciDev {
    pub bus: PciBus,
    pub num: u8,
}

impl<'pci> PciDev {
    pub fn funcs(self, pci: &'pci dyn CfgAccess) -> PciDevIter<'pci> {
        PciDevIter::new(self, pci)
    }
}

pub struct PciDevIter<'pci> {
    pci: &'pci dyn CfgAccess,
    dev: PciDev,
    num: u8,
}

impl<'pci> PciDevIter<'pci> {
    pub fn new(dev: PciDev, pci: &'pci dyn CfgAccess) -> Self {
        PciDevIter { pci, dev, num: 0 }
    }
}

impl<'pci> Iterator for PciDevIter<'pci> {
    type Item = PciFunc<'pci>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.num {
            func_num if func_num < 8 => {
                let func = PciFunc {
                    pci: self.pci,
                    addr: PciAddress::new(0, self.dev.bus.num, self.dev.num, func_num),
                };
                self.num += 1;
                Some(func)
            }
            _ => None,
        }
    }
}
