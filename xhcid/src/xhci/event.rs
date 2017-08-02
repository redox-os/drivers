use syscall::error::Result;
use syscall::io::{Dma, Io, Mmio};

use super::trb::Trb;

#[repr(packed)]
pub struct EventRingSte {
    pub address: Mmio<u64>,
    pub size: Mmio<u16>,
    _rsvd: Mmio<u16>,
    _rsvd2: Mmio<u32>,
}

pub struct EventRing {
    pub ste: Dma<EventRingSte>,
    pub trbs: Dma<[Trb; 256]>
}

impl EventRing {
    pub fn new() -> Result<EventRing> {
        let mut ring = EventRing {
            ste: Dma::zeroed()?,
            trbs: Dma::zeroed()?
        };

        ring.ste.address.write(ring.trbs.physical() as u64);
        ring.ste.size.write(ring.trbs.len() as u16);

        Ok(ring)
    }
}
