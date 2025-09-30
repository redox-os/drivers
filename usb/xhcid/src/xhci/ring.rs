use std::mem;

use syscall::error::Result;

use common::dma::Dma;

use super::trb::Trb;
use super::Xhci;

pub struct Ring {
    pub link: bool,
    pub trbs: Dma<[Trb]>,
    pub i: usize,
    pub cycle: bool,
}

impl Ring {
    pub fn new<const N: usize>(ac64: bool, length: usize, link: bool) -> Result<Ring> {
        Ok(Ring {
            link,
            trbs: unsafe { Xhci::<N>::alloc_dma_zeroed_unsized_raw(ac64, length)? },
            i: 0,
            cycle: link,
        })
    }

    pub fn register(&self) -> u64 {
        let base = self.trbs.physical() as *const Trb;
        let addr = unsafe { base.offset(self.i as isize) };
        addr as u64 | self.cycle as u64
    }

    pub fn next_index(&mut self) -> usize {
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
        i
    }

    pub fn next(&mut self) -> (&mut Trb, bool) {
        let i = self.next_index();
        (&mut self.trbs[i], self.cycle)
    }
    /// Endless iterator that iterates through the ring items, over and over again. The iterator
    /// doesn't enqueue or dequeue anything.
    pub fn iter(&self) -> impl Iterator<Item = &Trb> + '_ {
        Iter {
            ring: self,
            i: self.i,
        }
    }
    /// Takes a physical address and returns the index into this ring, that the index represents.
    /// Returns `None` if the address is outside the bounds of this ring.
    ///
    /// # Panics
    /// Panics if paddr is not a multiple of 16 bytes, i.e. the size of a TRB.
    pub fn phys_addr_to_index(&self, ac64: bool, paddr: u64) -> Option<usize> {
        let base = (self.trbs.physical() as u64)
            & if ac64 {
                0xFFFF_FFFF_FFFF_FFFF
            } else {
                0xFFFF_FFFF
            };
        let offset = paddr.checked_sub(base)? as usize;

        assert_eq!(
            offset % mem::size_of::<Trb>(),
            0,
            "unaligned TRB physical address"
        );

        let index = offset / mem::size_of::<Trb>();

        if index > self.trbs.len() {
            return None;
        }

        Some(index)
    }
    pub fn phys_addr_to_entry_ref(&self, ac64: bool, paddr: u64) -> Option<&Trb> {
        Some(&self.trbs[self.phys_addr_to_index(ac64, paddr)?])
    }
    pub fn phys_addr_to_entry_mut(&mut self, ac64: bool, paddr: u64) -> Option<&mut Trb> {
        let index = self.phys_addr_to_index(ac64, paddr)?;
        Some(&mut self.trbs[index])
    }
    pub fn phys_addr_to_entry(&self, ac64: bool, paddr: u64) -> Option<Trb> {
        Some(self.trbs[self.phys_addr_to_index(ac64, paddr)?].clone())
    }
    pub(crate) fn start_virt_addr(&self) -> *const Trb {
        self.trbs.as_ptr()
    }
    pub(crate) fn end_virt_addr(&self) -> *const Trb {
        unsafe { self.start_virt_addr().offset(self.trbs.len() as isize) }
    }
    pub fn trb_phys_ptr(&self, ac64: bool, trb: &Trb) -> u64 {
        let trb_virt_pointer = trb as *const Trb;
        let trbs_base_virt_pointer = self.trbs.as_ptr();

        if (trb_virt_pointer as usize) < (trbs_base_virt_pointer as usize)
            || (trb_virt_pointer as usize)
                > (trbs_base_virt_pointer as usize) + self.trbs.len() * mem::size_of::<Trb>()
        {
            panic!("Gave a TRB outside of the ring, when retrieving its physical address in that ring. TRB: {:?} (at address {:p})", trb, trb);
        }
        let trb_offset_from_base = trb_virt_pointer as u64 - trbs_base_virt_pointer as u64;

        let trbs_base_phys_ptr = (self.trbs.physical() as u64)
            & if ac64 {
                0xFFFF_FFFF_FFFF_FFFF
            } else {
                0xFFFF_FFFF
            };
        let trb_phys_ptr = trbs_base_phys_ptr + trb_offset_from_base;
        trb_phys_ptr
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
