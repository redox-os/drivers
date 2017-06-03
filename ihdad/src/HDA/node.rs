
use std::{mem, thread, ptr, fmt};
use super::common::*;

#[derive(Clone)]
pub struct HDANode {
	pub addr: HDANodeAddr,
	
	

	// 0x4
	pub subnode_count: u16,
	pub subnode_start: u16,
	
	// 0x5
	pub function_group_type: u8,


	// 0x9
	pub capabilities:     u32,

	// 0xC
	pub pin_caps:         u32,
	
	// 0xD
	pub in_amp: 	  u32,
	
	// 0xE
	pub conn_list_len:    u8,
	

	// 0x12
	pub out_amp:          u32,

	// 0x13
	pub vol_knob:         u8,
	
	pub connections:      Vec<HDANodeAddr>,
	
	pub is_widget:        bool,

	pub config_default:   u32,
}


impl HDANode {

	pub fn new() -> HDANode {
		HDANode {
			addr: 0,
			subnode_count: 0,
			subnode_start: 0,
			function_group_type: 0,
			capabilities: 0,
			pin_caps: 0,
			in_amp: 0,
			out_amp: 0,
			vol_knob: 0,
			conn_list_len: 0,

			config_default: 0,
			is_widget: false,
			connections: Vec::<HDANodeAddr>::new(),
		}
	}

	pub fn widget_type(&self) -> HDAWidgetType {
		unsafe { mem::transmute( ((self.capabilities >> 20) & 0xF) as u8 )}

	}

	pub fn getDeviceDefault(&self) -> Option<HDADefaultDevice> {
		if self.widget_type() != HDAWidgetType::PinComplex {
			None
		} else {
			Some(unsafe { mem::transmute( ((self.config_default >> 20) & 0xF) as u8 )} )
		}
	}
	
	pub fn addr(&self) -> HDANodeAddr {
		self.addr
	}
}

impl fmt::Display for HDANode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.addr == 0 {
		write!(f, "Addr: {:02X}, Root Node.", self.addr)
	} else if self.is_widget {
		match self.widget_type() {
		
		HDAWidgetType::PinComplex => { 
			write!(f, "Addr: {:02X}, Type: {:?}: {:?}, Inputs: {:X}: {:?}.", 
				self.addr, 
				self.widget_type(), 
				self.getDeviceDefault().unwrap(), 
				self.conn_list_len, 
				self.connections) 
			},
		
		  _ => { write!(f, "Addr: {:02X}, Type: {:?}, Inputs: {:X}: {:?}.", self.addr, self.widget_type(), self.conn_list_len, self.connections) },
		
		}
	} else {
		write!(f, "Addr: {:02X}, AFG: {}, Widget count {}.", self.addr, self.function_group_type, self.subnode_count)
	}
    }
}




