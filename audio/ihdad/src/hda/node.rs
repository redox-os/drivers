use super::common::*;
use std::{fmt, mem};

#[derive(Clone)]
pub struct HDANode {
    pub addr: WidgetAddr,

    // 0x4
    pub subnode_count: u16,
    pub subnode_start: u16,

    // 0x5
    pub function_group_type: u8,

    // 0x9
    pub capabilities: u32,

    // 0xE
    pub conn_list_len: u8,

    pub connections: Vec<WidgetAddr>,

    pub connection_default: u8,

    pub is_widget: bool,

    pub config_default: u32,
}

impl HDANode {
    pub fn new() -> HDANode {
        HDANode {
            addr: (0, 0),
            subnode_count: 0,
            subnode_start: 0,
            function_group_type: 0,
            capabilities: 0,
            conn_list_len: 0,

            config_default: 0,
            is_widget: false,
            connections: Vec::<WidgetAddr>::new(),
            connection_default: 0,
        }
    }

    pub fn widget_type(&self) -> HDAWidgetType {
        unsafe { mem::transmute(((self.capabilities >> 20) & 0xF) as u8) }
    }

    pub fn device_default(&self) -> Option<DefaultDevice> {
        if self.widget_type() != HDAWidgetType::PinComplex {
            None
        } else {
            Some(unsafe { mem::transmute(((self.config_default >> 20) & 0xF) as u8) })
        }
    }

    pub fn configuration_default(&self) -> ConfigurationDefault {
        ConfigurationDefault::from_u32(self.config_default)
    }

    pub fn addr(&self) -> WidgetAddr {
        self.addr
    }
}

impl fmt::Display for HDANode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.addr == (0, 0) {
            write!(
                f,
                "Addr: {:02X}:{:02X}, Root Node.",
                self.addr.0, self.addr.1
            )
        } else if self.is_widget {
            match self.widget_type() {
                HDAWidgetType::PinComplex => write!(
                    f,
                    "Addr: {:02X}:{:02X}, Type: {:?}: {:?}, Inputs: {}/{}: {:X?}.",
                    self.addr.0,
                    self.addr.1,
                    self.widget_type(),
                    self.device_default().unwrap(),
                    self.connection_default,
                    self.conn_list_len,
                    self.connections
                ),
                _ => write!(
                    f,
                    "Addr: {:02X}:{:02X}, Type: {:?}, Inputs: {}/{}: {:X?}.",
                    self.addr.0,
                    self.addr.1,
                    self.widget_type(),
                    self.connection_default,
                    self.conn_list_len,
                    self.connections
                ),
            }
        } else {
            write!(
                f,
                "Addr: {:02X}:{:02X}, AFG: {}, Widget count {}.",
                self.addr.0, self.addr.1, self.function_group_type, self.subnode_count
            )
        }
    }
}
