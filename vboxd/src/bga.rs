use common::io::{Io, Pio};

const BGA_INDEX_XRES: u16 = 1;
const BGA_INDEX_YRES: u16 = 2;
const BGA_INDEX_BPP: u16 = 3;
const BGA_INDEX_ENABLE: u16 = 4;

pub struct Bga {
    index: Pio<u16>,
    data: Pio<u16>,
}

impl Bga {
    pub fn new() -> Bga {
        Bga {
            index: Pio::new(0x1CE),
            data: Pio::new(0x1CF),
        }
    }

    fn read(&mut self, index: u16) -> u16 {
        self.index.write(index);
        self.data.read()
    }

    fn write(&mut self, index: u16, data: u16) {
        self.index.write(index);
        self.data.write(data);
    }

    pub fn width(&mut self) -> u16 {
        self.read(BGA_INDEX_XRES)
    }

    pub fn height(&mut self) -> u16 {
        self.read(BGA_INDEX_YRES)
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.write(BGA_INDEX_ENABLE, 0);
        self.write(BGA_INDEX_XRES, width);
        self.write(BGA_INDEX_YRES, height);
        self.write(BGA_INDEX_BPP, 32);
        self.write(BGA_INDEX_ENABLE, 0x41);
    }
}
