use std::{error::Error, fs};

use super::Backend;

pub struct DeviceTreeBackend {
    dtb: Vec<u8>,
}

impl DeviceTreeBackend {
    fn dump(node: &fdt::node::FdtNode<'_, '_>, level: usize) {
        let mut line = String::new();
        for _ in 0..level {
            line.push_str("  ");
        }
        line.push_str(node.name);
        if let Some(compatible) = node.compatible() {
            line.push_str(":");
            for id in compatible.all() {
                line.push_str(" ");
                line.push_str(id);
            }
        }
        log::debug!("{}", line);
        for child in node.children() {
            Self::dump(&child, level + 1);
        }
    }
}

impl Backend for DeviceTreeBackend {
    fn new() -> Result<Self, Box<dyn Error>> {
        let dtb = fs::read("/scheme/kernel.dtb")?;
        let dt = fdt::Fdt::new(&dtb).map_err(|err| format!("failed to parse dtb: {}", err))?;
        Ok(Self { dtb })
    }

    fn probe(&mut self) -> Result<(), Box<dyn Error>> {
        let dt = fdt::Fdt::new(&self.dtb).map_err(|err| format!("failed to parse dtb: {}", err))?;
        let root = dt
            .find_node("/")
            .ok_or_else(|| format!("failed to find root node"))?;
        Self::dump(&root, 0);
        Ok(())
    }
}
