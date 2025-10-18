use std::error::Error;

mod acpi;
mod devicetree;
mod legacy;

pub use self::{acpi::AcpiBackend, devicetree::DeviceTreeBackend, legacy::LegacyBackend};

pub trait Backend {
    fn new() -> Result<Self, Box<dyn Error>>
    where
        Self: Sized;
    fn probe(&mut self) -> Result<(), Box<dyn Error>>;
}
