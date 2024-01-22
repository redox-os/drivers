use serde::{Deserialize, Serialize};

/// All identifying information of a PCI function.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FullDeviceId {
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub interface: u8,
    pub revision: u8,
}
