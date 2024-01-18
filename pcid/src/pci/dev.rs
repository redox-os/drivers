use super::{PciBus, PciFunc};

pub struct PciDev<'pci> {
    pub bus: &'pci PciBus<'pci>,
    pub num: u8,
}

impl<'pci> PciDev<'pci> {
    pub fn funcs(&'pci self) -> PciDevIter<'pci> {
        PciDevIter::new(self)
    }
}

pub struct PciDevIter<'pci> {
    dev: &'pci PciDev<'pci>,
    num: u8,
}

impl<'pci> PciDevIter<'pci> {
    pub fn new(dev: &'pci PciDev<'pci>) -> Self {
        PciDevIter { dev, num: 0 }
    }
}

impl<'pci> Iterator for PciDevIter<'pci> {
    type Item = PciFunc<'pci>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.num {
            func_num if func_num < 8 => {
                let func = PciFunc {
                    dev: self.dev,
                    num: self.num,
                };
                self.num += 1;
                Some(func)
            }
            _ => None,
        }
    }
}
