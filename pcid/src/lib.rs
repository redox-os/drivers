//! Interface to `pcid`.

#![feature(asm)]

mod driver_interface;
mod pci;
pub use driver_interface::*;
