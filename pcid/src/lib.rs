//! Interface to `pcid`.

pub mod driver_interface;
pub mod pci;
pub mod pcie;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PciAddr {
    pub seg: u16,
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}
