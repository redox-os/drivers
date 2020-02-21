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
    /// Endless iterator that iterates through the ring items, over and over again. The iterator
    /// doesn't enqueue or dequeue anything.
    pub fn iter(&self) -> impl Iterator<Item = &Trb> + '_ {
        Iter { ring: self, i: self.i }
    }
    /*
    /// Endless mutable iterator that iterates through the ring items, over and over again. The
    /// iterator doesn't enqueue or dequeue anything, but the trbs are mutably borrowed.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Trb> + '_ {
        IterMut { ring: self, i: self.i }
    }*/
}
struct Iter<'ring> {
    ring: &'ring Ring,
    i: usize,

}
impl<'ring> Iterator for Iter<'ring> {
    type Item = &'ring Trb;

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.i;
        self.i = (self.i + 1) % self.ring.trbs.len();
        Some(&self.ring.trbs[i])
    }
}
/*struct IterMut<'ring> {
    ring: &'ring mut Ring,
    i: usize,
}
impl<'ring> Iterator for IterMut<'ring> {
    type Item = &'ring mut Trb;

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.i;
        self.i = (self.i + 1) % self.ring.trbs.len();
        Some(&mut self.ring.trbs[i])
    }
}*/
