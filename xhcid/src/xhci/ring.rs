use syscall::error::Result;
use syscall::io::Dma;

use super::trb::Trb;

pub struct Ring {
    pub link: bool,
    pub trbs: Dma<[Trb; 16]>,
    pub i: usize,
    pub cycle: bool,
}

impl Ring {
    pub fn new(link: bool) -> Result<Ring> {
        Ok(Ring {
            link: link,
            trbs: Dma::zeroed()?,
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
                    println!("Link");
                    let address = self.trbs.physical();
                    self.trbs[i].link(address, true, self.cycle);
                    self.cycle = !self.cycle;
                } else {
                    println!("No-link");
                    break;
                }
            } else {
                break;
            }
        }

        (&mut self.trbs[i], self.cycle)
    }
}
