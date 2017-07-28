use syscall::error::Result;
use syscall::io::Dma;

use super::event::EventRing;
use super::trb::Trb;

pub struct CommandRing {
    pub trbs: Dma<[Trb; 256]>,
    pub events: EventRing,
}

impl CommandRing {
    pub fn new() -> Result<CommandRing> {
        Ok(CommandRing {
            trbs: Dma::zeroed()?,
            events: EventRing::new()?,
        })
    }

    pub fn crcr(&self) -> u64 {
        self.trbs.physical() as u64 | 1
    }
}
