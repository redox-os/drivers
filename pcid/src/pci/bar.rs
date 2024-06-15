use std::convert::TryInto;

use serde::{Deserialize, Serialize};

// This type is used instead of [pci_types::Bar] in the driver interface as the
// latter can't be serialized and is missing the convenience functions of [PciBar].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PciBar {
    None,
    Memory32 { addr: u32, size: u32 },
    Memory64 { addr: u64, size: u64 },
    Port(u16),
}

impl PciBar {
    pub fn display(&self) -> String {
        match self {
            PciBar::None => format!("<none>"),
            PciBar::Memory32 { addr, .. } => format!("{addr:08X}"),
            PciBar::Memory64 { addr, .. } => format!("{addr:016X}"),
            PciBar::Port(port) => format!("P{port:04X}"),
        }
    }

    pub fn is_none(&self) -> bool {
        match self {
            &PciBar::None => true,
            _ => false,
        }
    }

    pub fn expect_port(&self) -> u16 {
        match *self {
            PciBar::Port(port) => port,
            PciBar::Memory32 { .. } | PciBar::Memory64 { .. } => {
                panic!("expected port BAR, found memory BAR");
            }
            PciBar::None => panic!("expected BAR to exist"),
        }
    }

    pub fn expect_mem(&self) -> (usize, usize) {
        match *self {
            PciBar::Memory32 { addr, size } => (addr as usize, size as usize),
            PciBar::Memory64 { addr, size } => (
                addr.try_into()
                    .expect("conversion from 64bit BAR to usize failed"),
                size.try_into()
                    .expect("conversion from 64bit BAR size to usize failed"),
            ),
            PciBar::Port(_) => panic!("expected memory BAR, found port BAR"),
            PciBar::None => panic!("expected BAR to exist"),
        }
    }
}
