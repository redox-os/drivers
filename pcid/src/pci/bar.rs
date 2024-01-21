use std::convert::TryInto;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PciBar {
    None,
    Memory32(u32),
    Memory64(u64),
    Port(u16),
}

impl PciBar {
    pub fn is_none(&self) -> bool {
        match self {
            &PciBar::None => true,
            _ => false,
        }
    }

    pub fn expect_port(&self) -> u16 {
        match *self {
            PciBar::Port(port) => port,
            PciBar::Memory32(_) | PciBar::Memory64(_) => {
                panic!("expected port BAR, found memory BAR");
            }
            PciBar::None => panic!("expected BAR to exist"),
        }
    }

    pub fn expect_mem(&self) -> usize {
        match *self {
            PciBar::Memory32(ptr) => ptr as usize,
            PciBar::Memory64(ptr) => ptr
                .try_into()
                .expect("conversion from 64bit BAR to usize failed"),
            PciBar::Port(_) => panic!("expected memory BAR, found port BAR"),
            PciBar::None => panic!("expected BAR to exist"),
        }
    }
}

impl From<u32> for PciBar {
    fn from(bar: u32) -> Self {
        if bar & 0xFFFFFFFC == 0 {
            PciBar::None
        } else if bar & 1 == 0 {
            match (bar >> 1) & 3 {
                0 => PciBar::Memory32(bar & 0xFFFFFFF0),
                2 => PciBar::Memory64((bar & 0xFFFFFFF0) as u64),
                other => {
                    log::warn!("unsupported PCI memory type {}", other);
                    PciBar::None
                }
            }
        } else {
            PciBar::Port((bar & 0xFFFC) as u16)
        }
    }
}
