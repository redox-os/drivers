use syscall::error::Result;
use syscall::io::Dma;

use super::event::EventRing;
use super::ring::Ring;
use super::trb::Trb;

pub struct CommandRing {
    pub ring: Ring,
    pub events: EventRing,
}

impl CommandRing {
    pub fn new() -> Result<CommandRing> {
        Ok(CommandRing {
            ring: Ring::new(true)?,
            events: EventRing::new()?,
        })
    }

    pub fn crcr(&self) -> u64 {
        self.ring.register()
    }

    pub fn erdp(&self) -> u64 {
        self.events.ring.register()
    }

    pub fn erstba(&self) -> u64 {
        self.events.ste.physical() as u64
    }

    pub fn next(&mut self) -> (&mut Trb, bool, &mut Trb) {
        let cmd = self.ring.next();
        let event = self.events.next();
        (cmd.0, cmd.1, event)
    }

    pub fn next_cmd(&mut self) -> (&mut Trb, bool) {
        self.ring.next()
    }

    pub fn next_event(&mut self) -> &mut Trb {
        self.events.next()
    }
}
