use syscall::error::Result;
use syscall::io::Dma;

use super::trb::Trb;

pub struct Ring {
    pub link: bool,
    pub trbs: Dma<[Trb]>,
    pub i: usize,
    pub cycle: bool,
}

impl Ring {
    pub fn new(length: usize, link: bool) -> Result<Ring> {
        Ok(Ring {
            link: link,
            trbs: unsafe { Dma::zeroed_unsized(length)? },
            i: 0,
            cycle: link,
        })
    }

    pub fn register(&self) -> u64 {
        let base = self.trbs.physical() as *const Trb;
        let addr = unsafe { base.offset(self.i as isize) };
        addr as u64 | self.cycle as u64
    }

    pub fn next(&mut self) -> (&mut Trb, bool) {
        let mut i;
        loop {
            i = self.i;
            self.i += 1;
            if self.i >= self.trbs.len() {
                self.i = 0;

                if self.link {
                    let address = self.trbs.physical();
                    self.trbs[i].link(address, true, self.cycle);
                    self.cycle = !self.cycle;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        (&mut self.trbs[i], self.cycle)
    }
}
