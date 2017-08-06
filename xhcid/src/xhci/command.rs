use syscall::error::Result;
use syscall::io::Dma;

use super::event::EventRing;
use super::trb::Trb;

pub struct CommandRing {
    trbs: Dma<[Trb; 256]>,
    cmd_i: usize,
    pub events: EventRing,
    event_i: usize,
}

impl CommandRing {
    pub fn new() -> Result<CommandRing> {
        Ok(CommandRing {
            trbs: Dma::zeroed()?,
            cmd_i: 0,
            events: EventRing::new()?,
            event_i: 0,
        })
    }

    pub fn crcr(&self) -> u64 {
        self.trbs.physical() as u64 | 1
    }

    pub fn next(&mut self) -> (&mut Trb, &mut Trb) {
        let cmd_i = self.cmd_i;
        self.cmd_i += 1;
        if self.cmd_i >= self.trbs.len() {
            self.cmd_i = 0;
        }

        let event_i = self.event_i;
        self.event_i += 1;
        if self.event_i >= self.events.trbs.len() {
            self.event_i = 0;
        }

        (&mut self.trbs[cmd_i], &mut self.events.trbs[event_i])
    }

    pub fn next_cmd(&mut self) -> &mut Trb {
        let i = self.cmd_i;
        self.cmd_i += 1;
        if self.cmd_i >= self.trbs.len() {
            self.cmd_i = 0;
        }

        &mut self.trbs[i]
    }

    pub fn next_event(&mut self) -> &mut Trb {
        let i = self.event_i;
        self.event_i += 1;
        if self.event_i >= self.events.trbs.len() {
            self.event_i = 0;
        }

        &mut self.events.trbs[i]
    }
}
