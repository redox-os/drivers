//! Interface to `pcid`.

#![feature(llvm_asm)]

mod driver_interface;
mod pci;
pub use driver_interface::*;
