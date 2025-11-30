use common::io::{Io, MmioPtr};
use syscall::error::Result;

use super::MmioRegion;

pub struct Plane {
    pub name: &'static str,
    pub index: usize,
    pub ctl: MmioPtr<u32>,
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
            eprint!("  Plane {}", plane.name);
            eprint!(" ctl {:08X}", plane.ctl.read());
            eprintln!();
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