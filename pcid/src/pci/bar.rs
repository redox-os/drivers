use serde::{Serialize, Deserialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PciBar {
    None,
    Memory32(u32),
    Memory64(u64),
    Port(u16)
}

impl PciBar {
    pub fn is_none(&self) -> bool {
        match self {
            &PciBar::None => true,
            _ => false,
        }
    }
}

impl From<u32> for PciBar {
    fn from(bar: u32) -> Self {
        if bar & 0xFFFFFFFC == 0 {
            PciBar::None
        } else if bar & 1 == 0 {
            match (bar >> 1) & 3 {
                0 => {
                    PciBar::Memory32(bar & 0xFFFFFFF0)
                },
                2 => {
                    PciBar::Memory64((bar & 0xFFFFFFF0) as u64)
                },
                other => {
                    log::warn!("unsupported PCI memory type {}", other);
                    PciBar::None
                },
            }
        } else {
            PciBar::Port((bar & 0xFFFC) as u16)
        }
    }
}
