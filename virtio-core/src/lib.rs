pub mod spec;
pub mod transport;
pub mod utils;

mod probe;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64.rs"]
mod arch;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[path = "arch/x86.rs"]
mod arch;

#[cfg(target_arch = "riscv64")]
#[path = "arch/riscv64.rs"]
mod arch;

pub use probe::{probe_device, reinit, Device, MSIX_PRIMARY_VECTOR};
