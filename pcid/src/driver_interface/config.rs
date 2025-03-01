use std::collections::BTreeMap;
use std::ops::Range;

use serde::Deserialize;

use crate::driver_interface::FullDeviceId;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Config {
    pub drivers: Vec<DriverConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct DriverConfig {
    pub name: Option<String>,
    pub class: Option<u8>,
    pub subclass: Option<u8>,
    pub interface: Option<u8>,
    pub ids: Option<BTreeMap<String, Vec<u16>>>,
    pub vendor: Option<u16>,
    pub device: Option<u16>,
    pub device_id_range: Option<Range<u16>>,
    pub command: Vec<String>,
}

impl DriverConfig {
    pub fn match_function(&self, id: &FullDeviceId) -> bool {
        if let Some(class) = self.class {
            if class != id.class {
                return false;
            }
        }

        if let Some(subclass) = self.subclass {
            if subclass != id.subclass {
                return false;
            }
        }

        if let Some(interface) = self.interface {
            if interface != id.interface {
                return false;
            }
        }

        if let Some(ref ids) = self.ids {
            let mut device_found = false;
            for (vendor, devices) in ids {
                let vendor_without_prefix = vendor.trim_start_matches("0x");
                let vendor = i64::from_str_radix(vendor_without_prefix, 16).unwrap() as u16;

                if vendor != id.vendor_id {
                    continue;
                }

                for device in devices {
                    if *device == id.device_id {
                        device_found = true;
                        break;
                    }
                }
            }
            if !device_found {
                return false;
            }
        } else {
            if let Some(vendor) = self.vendor {
                if vendor != id.vendor_id {
                    return false;
                }
            }

            if let Some(device) = self.device {
                if device != id.device_id {
                    return false;
                }
            }
        }

        if let Some(ref device_id_range) = self.device_id_range {
            if id.device_id < device_id_range.start || device_id_range.end <= id.device_id {
                return false;
            }
        }

        true
    }
}
