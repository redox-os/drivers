#![feature(int_roundings)]

pub mod spec;
pub mod transport;
pub mod utils;

mod probe;

#[cfg(target_arch = "aarch64")]
#[path="arch/aarch64.rs"]
mod arch;

#[cfg(target_arch = "x86")]
#[path="arch/x86.rs"]
mod arch;

#[cfg(target_arch = "x86_64")]
#[path="arch/x86_64.rs"]
mod arch;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod legacy_transport;


pub use probe::{probe_device, reinit, Device, MSIX_PRIMARY_VECTOR};
