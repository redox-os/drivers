use common::io::{Io, Mmio};
use syscall::error::Result;

use common::dma::Dma;

use super::ring::Ring;
use super::trb::Trb;
use super::Xhci;

#[repr(C, packed)]
pub struct EventRingSte {
    pub address_low: Mmio<u32>,
    pub address_high: Mmio<u32>,
    pub size: Mmio<u16>,
    _rsvd: Mmio<u16>,
    _rsvd2: Mmio<u32>,
}

// TODO: Use atomic operations, and perhaps an occasional lock for reallocating.
pub struct EventRing {
    pub ste: Dma<[EventRingSte]>,
    pub ring: Ring,
}

impl EventRing {
    pub fn new<const N: usize>(ac64: bool) -> Result<EventRing> {
        let mut ring = EventRing {
            ste: unsafe { Xhci::<N>::alloc_dma_zeroed_unsized_raw(ac64, 1)? },
            ring: Ring::new::<N>(ac64, 256, false)?,
        };

        ring.ste[0]
            .address_low
            .write(ring.ring.trbs.physical() as u32);
        ring.ste[0]
            .address_high
            .write((ring.ring.trbs.physical() as u64 >> 32) as u32);
        ring.ste[0].size.write(ring.ring.trbs.len() as u16);

        Ok(ring)
    }

    pub fn next(&mut self) -> &mut Trb {
        self.ring.next().0
    }
    pub fn erdp(&self) -> u64 {
        self.ring.register() & 0xFFFF_FFFF_FFFF_FFF0
    }
    pub fn erstba(&self) -> u64 {
        self.ste.physical() as u64
    }
}
