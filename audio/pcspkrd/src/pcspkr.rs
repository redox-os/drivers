use common::io::{Io, Pio};

pub struct Pcspkr {
    command: Pio<u8>,
    channel: Pio<u8>,
    gate: Pio<u8>,
}

const PIT_FREQUENCY: usize = 0x1234DC;

impl Pcspkr {
    pub fn new() -> Pcspkr {
        Pcspkr {
            command: Pio::new(0x43),
            channel: Pio::new(0x42),
            gate: Pio::new(0x61),
        }
    }

    pub fn set_frequency(&mut self, frequency: usize) {
        let div = PIT_FREQUENCY.checked_div(frequency).unwrap_or(0);
        self.command.write(0xB6);
        self.channel.write((div & 0xFF) as u8);
        self.channel.write(((div >> 8) & 0xFF) as u8);
    }

    pub fn set_gate(&mut self, state: bool) {
        let gate_value = self.gate.read();
        if state {
            self.gate.write(gate_value | 0x03);
        } else {
            self.gate.write(gate_value & 0xFC);
        }
    }
}
