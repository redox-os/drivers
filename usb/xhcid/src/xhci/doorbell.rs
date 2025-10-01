use common::io::{Io, Mmio};

#[repr(C, packed)]
pub struct Doorbell(Mmio<u32>);

impl Doorbell {
    pub fn read(&self) -> u32 {
        self.0.read()
    }

    pub fn write(&mut self, data: u32) {
        self.0.write(data);
    }
}
