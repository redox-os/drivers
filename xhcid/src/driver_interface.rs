pub extern crate serde;
pub extern crate smallvec;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use syscall::{Error, Result, EINVAL};

pub use crate::usb::{EndpointTy, ENDP_ATTR_TY_MASK};

#[derive(Serialize, Deserialize)]
pub struct ConfigureEndpointsReq {
    /// Index into the configuration descriptors of the device descriptor.
    pub config_desc: usize,
    // TODO: Support multiple alternate interfaces as well.
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevDesc {
    pub kind: u8,
    pub usb: u16,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub packet_size: u8,
    pub vendor: u16,
    pub product: u16,
    pub release: u16,
    pub manufacturer_str: Option<String>,
    pub product_str: Option<String>,
    pub serial_str: Option<String>,
    pub config_descs: SmallVec<[ConfDesc; 1]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfDesc {
    pub kind: u8,
    pub configuration_value: u8,
    pub configuration: Option<String>,
    pub attributes: u8,
    pub max_power: u8,
    pub interface_descs: SmallVec<[IfDesc; 1]>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EndpDesc {
    pub kind: u8,
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
    pub ssc: Option<SuperSpeedCmp>,
}
pub enum EndpDirection {
    Out,
    In,
    Bidirectional,
}

impl EndpDesc {
    pub fn ty(self) -> EndpointTy {
        match self.attributes & ENDP_ATTR_TY_MASK {
            0 => EndpointTy::Ctrl,
            1 => EndpointTy::Interrupt,
            2 => EndpointTy::Bulk,
            3 => EndpointTy::Isoch,
            _ => unreachable!(),
        }
    }
    pub fn is_control(&self) -> bool {
        self.ty() == EndpointTy::Ctrl
    }
    pub fn is_interrupt(&self) -> bool {
        self.ty() == EndpointTy::Interrupt
    }
    pub fn is_bulk(&self) -> bool {
        self.ty() == EndpointTy::Bulk
    }
    pub fn is_isoch(&self) -> bool {
        self.ty() == EndpointTy::Isoch
    }
    pub fn direction(&self) -> EndpDirection {
        if self.is_control() {
            return EndpDirection::Bidirectional;
        }
        if self.address & 0x80 != 0 {
            EndpDirection::In
        } else {
            EndpDirection::Out
        }
    }
    pub fn xhci_ep_type(&self) -> Result<u8> {
        Ok(match self.direction() {
            EndpDirection::Out if self.is_isoch() => 1,
            EndpDirection::Out if self.is_bulk() => 2,
            EndpDirection::Out if self.is_interrupt() => 3,
            EndpDirection::Bidirectional if self.is_control() => 4,
            EndpDirection::In if self.is_isoch() => 5,
            EndpDirection::In if self.is_bulk() => 6,
            EndpDirection::In if self.is_interrupt() => 7,
            _ => return Err(Error::new(EINVAL)),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IfDesc {
    pub kind: u8,
    pub number: u8,
    pub alternate_setting: u8,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub interface_str: Option<String>,
    pub endpoints: SmallVec<[EndpDesc; 4]>,
    pub hid_descs: SmallVec<[HidDesc; 1]>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SuperSpeedCmp {
    pub kind: u8,
    pub max_burst: u8,
    pub attributes: u8,
    pub bytes_per_interval: u16,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct HidDesc {
    pub kind: u8,
    pub hid_spec_release: u16,
    pub country: u8,
    pub desc_count: u8,
    pub desc_ty: u8,
    pub desc_len: u16,
    pub optional_desc_ty: u8,
    pub optional_desc_len: u16,
}
