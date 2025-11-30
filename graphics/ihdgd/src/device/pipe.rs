use common::io::{Io, MmioPtr};
use syscall::error::Result;

use super::MmioRegion;

pub struct Plane {
    pub name: &'static str,
    pub index: usize,
    pub ctl: MmioPtr<u32>,
    pub offset: MmioPtr<u32>,
    pub pos: MmioPtr<u32>,
    pub size: MmioPtr<u32>,
    pub stride: MmioPtr<u32>,
    pub surf: MmioPtr<u32>,
}

impl Plane {
    pub fn dump(&self) {
        eprint!("Plane {}", self.name);
        eprint!(" ctl {:08X}", self.ctl.read());
        eprint!(" offset {:08X}", self.offset.read());
        eprint!(" pos {:08X}", self.offset.read());
        eprint!(" size {:08X}", self.size.read());
        eprint!(" stride {:08X}", self.stride.read());
        eprint!(" surf {:08X}", self.surf.read());
        eprintln!();
    }
}

pub struct Pipe {
    pub name: &'static str,
    pub index: usize,
    pub planes: Vec<Plane>,
    pub misc: MmioPtr<u32>,
    pub srcsz: MmioPtr<u32>,
}

impl Pipe {
    pub fn dump(&self) {
        eprint!("Pipe {}", self.name);
        eprint!(" misc {:08X}", self.misc.read());
        eprint!(" srcsz {:08X}", self.srcsz.read());
        eprintln!();
        for plane in self.planes.iter() {
            eprint!("  ");
            plane.dump();
        }
    }

    pub fn tigerlake(gttmm: &MmioRegion) -> Result<Vec<Self>> {
        let mut pipes = Vec::with_capacity(4);
        for (i, name) in ["A", "B", "C", "D"].iter().enumerate() {
            let mut planes = Vec::new();
            for (j, name) in ["1", "2", "3", "4", "5", "6", "7"].iter().enumerate() {
                planes.push(Plane {
                    name,
                    index: j,
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_CTL
                    ctl: unsafe { gttmm.mmio(0x70180 + i * 0x1000 + j * 0x100)? },
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_OFFSET
                    offset: unsafe { gttmm.mmio(0x701A4 + i * 0x1000 + j * 0x100)? },
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_POS
                    pos: unsafe { gttmm.mmio(0x7018C + i * 0x1000 + j * 0x100)? },
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_SIZE
                    size: unsafe { gttmm.mmio(0x70190 + i * 0x1000 + j * 0x100)? },
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_STRIDE
                    stride: unsafe { gttmm.mmio(0x70188 + i * 0x1000 + j * 0x100)? },
                    // IHD-OS-TGL-Vol 2c-12.21 PLANE_SURF
                    surf: unsafe { gttmm.mmio(0x7019C + i * 0x1000 + j * 0x100)? },
                });
            }
            pipes.push(Pipe {
                name,
                index: i,
                planes,
                // IHD-OS-TGL-Vol 2c-12.21 PIPE_MISC
                misc: unsafe { gttmm.mmio(0x70030 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 PIPE_SRCSZ
                srcsz: unsafe { gttmm.mmio(0x6001C + i * 0x1000)? },
            })
        }
        Ok(pipes)
    }
}