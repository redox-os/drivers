pub extern crate serde;
pub extern crate smallvec;

use std::fs::File;
use std::io::prelude::*;
use std::{io, result, str};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use syscall::{Error, Result, EINVAL};
use thiserror::Error;

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
    pub fn max_streams(&self) -> u8 {
        self.ssc
            .as_ref()
            .map(|ssc| {
                if self.is_bulk() {
                    1 << (ssc.attributes & 0x1F)
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }
    pub fn isoch_mult(&self, lec: bool) -> u8 {
        if !lec && self.is_isoch() {
            self.ssc
                .as_ref()
                .map(|ssc| ssc.attributes & 0x3)
                .unwrap_or(0)
        } else {
            0
        }
    }
    pub fn max_burst(&self) -> u8 {
        self.ssc.map(|ssc| ssc.max_burst).unwrap_or(0)
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PortReq<'a> {
    pub direction: &'a str,
    pub req_type: &'a str,
    pub req_recipient: &'a str,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    pub transfers_data: bool,
}

#[derive(Debug)]
pub struct XhciClientHandle {
    scheme: String,
    port: usize,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PortState {
    EnabledOrDisabled,
    Default,
    Addressed,
    Configured,
}
impl PortState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EnabledOrDisabled => "enabled_or_disabled",
            Self::Default => "default",
            Self::Addressed => "addressed",
            Self::Configured => "configured",
        }
    }
}
#[derive(Debug, Error)]
#[error("invalid input")]
pub struct Invalid;

impl str::FromStr for PortState {
    type Err = Invalid;

    fn from_str(s: &str) -> result::Result<Self, Self::Err> {
        Ok(match s {
            "enabled_or_disabled" | "enabled/disabled" => Self::EnabledOrDisabled,
            "default" => Self::Default,
            "addressed" => Self::Addressed,
            "configured" => Self::Configured,
            _ => return Err(Invalid),
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndpointStatus {
    Disabled,
    Enabled,
    Halted,
    Stopped,
    Error,
}

impl EndpointStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
            Self::Halted => "halted",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}

impl str::FromStr for EndpointStatus {
    type Err = Invalid;

    fn from_str(s: &str) -> result::Result<Self, Self::Err> {
        Ok(match s {
            "disabled" => Self::Disabled,
            "enabled" => Self::Enabled,
            "halted" => Self::Halted,
            "stopped" => Self::Stopped,
            "error" => Self::Error,
            _ => return Err(Invalid),
        })
    }
}

impl XhciClientHandle {
    pub fn new(scheme: String, port: usize) -> Self {
        Self { scheme, port }
    }

    pub fn get_standard_descs(&self) -> result::Result<DevDesc, XhciClientHandleError> {
        let path = format!("{}:port{}/descriptors", self.scheme, self.port);
        let json = std::fs::read(path)?;
        Ok(serde_json::from_slice(&json)?)
    }
    pub fn configure_endpoints(
        &self,
        req: &ConfigureEndpointsReq,
    ) -> result::Result<(), XhciClientHandleError> {
        let path = format!("{}:port{}/configure_endpoints", self.scheme, self.port);
        serde_json::to_writer(File::open(path)?, req)?;
        Ok(())
    }
    pub fn port_state(&self) -> result::Result<PortState, XhciClientHandleError> {
        let path = format!("{}:port{}/state", self.scheme, self.port);
        let string = std::fs::read_to_string(path)?;
        Ok(string.parse()?)
    }
    pub fn endpoint_status(
        &self,
        num: u8,
    ) -> result::Result<EndpointStatus, XhciClientHandleError> {
        let path = format!("{}:port{}/endpoints/{}/status", self.scheme, self.port, num);
        let string = std::fs::read_to_string(path)?;
        Ok(string.parse()?)
    }
    // TODO: Device-specific request, with data
    pub fn get_class_descriptor(
        &self,
        value: u16,
        index: u16,
        length: u16,
    ) -> result::Result<Vec<u8>, XhciClientHandleError> {
        // TODO: Base this on the to-be-written generic device request function.
        let req = PortReq {
            direction: "device_to_host",
            req_type: "standard", // TODO: Add as a parameter, with its own enum.
            req_recipient: "interface",
            request: 0x06,
            value,
            index,
            length,
            transfers_data: true,
        };
        let json = serde_json::to_vec(&req)?;

        let path = format!("{}:port{}/request", self.scheme, self.port);
        let mut file = File::open(path)?;

        let bytes_written = file.write(&json)?;
        if bytes_written != json.len() {
            return Err(XhciClientHandleError::InvalidResponse(Invalid));
        }

        let mut buf = vec![0u8; length as usize];
        let bytes_read = file.read(&mut buf)?;
        if bytes_read != buf.len() {
            println!("HIDd B");
            return Err(XhciClientHandleError::InvalidResponse(Invalid));
        }

        Ok(buf)
    }
}

#[derive(Debug, Error)]
pub enum XhciClientHandleError {
    #[error("i/o error: {0}")]
    IoError(#[from] io::Error),

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("invalid response")]
    InvalidResponse(#[from] Invalid),
}
