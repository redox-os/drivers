use std::{error::Error, fs};

use super::Backend;

pub struct DeviceTreeBackend {
    dtb: Vec<u8>,
}

impl Backend for DeviceTreeBackend {
    fn new() -> Result<Self, Box<dyn Error>> {
        let dtb = fs::read("/scheme/kernel.dtb")?;
        let dt = fdt::Fdt::new(&dtb).map_err(|err| format!("failed to parse dtb: {}", err))?;
        Ok(Self { dtb })
    }

    fn probe(&mut self) -> Result<(), Box<dyn Error>> {
        let dt = fdt::Fdt::new(&self.dtb).map_err(|err| format!("failed to parse dtb: {}", err))?;
        log::info!("TODO: handle driver spawning from devicetree backend");
        Ok(())
    }
}
