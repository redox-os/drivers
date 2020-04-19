use syscall::error::Result;
use syscall::io::{Dma, Io, Mmio};

use super::ring::Ring;
use super::trb::Trb;

#[repr(packed)]
pub struct EventRingSte {
    pub address: Mmio<u64>,
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
    pub fn new() -> Result<EventRing> {
        let mut ring = EventRing {
            ste: unsafe { Dma::zeroed_unsized(1)? },
            ring: Ring::new(256, false)?,
        };

        ring.ste[0].address.write(ring.ring.trbs.physical() as u64);
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
