use super::PciDev;

#[derive(Copy, Clone)]
pub struct PciBus {
    pub num: u8,
}

impl<'pci> PciBus {
    pub fn devs(self) -> PciBusIter {
        PciBusIter::new(self)
    }
}

pub struct PciBusIter {
    bus: PciBus,
    num: u8,
}

impl PciBusIter {
    pub fn new(bus: PciBus) -> Self {
        PciBusIter { bus, num: 0 }
    }
}

impl Iterator for PciBusIter {
    type Item = PciDev;
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
