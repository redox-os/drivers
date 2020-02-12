pub extern crate serde;
pub extern crate smallvec;

use std::convert::TryFrom;
use std::fs::{File, OpenOptions};
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
    pub config_desc: u8,
    pub interface_desc: Option<u8>,
    pub alternate_setting: Option<u8>,
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

impl DevDesc {
    pub fn major_version(&self) -> u8 {
        ((self.usb & 0xFF00) >> 8) as u8
    }
    pub fn minor_version(&self) -> u8 {
        self.usb as u8
    }
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
    pub sspc: Option<SuperSpeedPlusIsochCmp>,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndpDirection {
    Out,
    In,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndpBinaryDirection {
    Out,
    In,
}

impl From<PortReqDirection> for EndpBinaryDirection {
    fn from(d: PortReqDirection) -> Self {
        match d {
            PortReqDirection::DeviceToHost => Self::In,
            PortReqDirection::HostToDevice => Self::Out,
        }
    }
}

impl From<EndpBinaryDirection> for EndpDirection {
    fn from(b: EndpBinaryDirection) -> Self {
        match b {
            EndpBinaryDirection::In => Self::In,
            EndpBinaryDirection::Out => Self::Out,
        }
    }
}

impl From<PortReqDirection> for EndpDirection {
    fn from(d: PortReqDirection) -> Self {
        match d {
            PortReqDirection::HostToDevice => Self::Out,
            PortReqDirection::DeviceToHost => Self::In,
        }
    }
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
    pub fn is_superspeed(&self) -> bool {
        self.ssc.is_some()
    }
    pub fn is_superspeedplus(&self) -> bool {
        todo!()
    }
    fn interrupt_usage_bits(&self) -> u8 {
        assert!(self.is_interrupt());
        (self.attributes & 0x20) >> 4
    }
    pub fn is_periodic(&self) -> bool {
        #[repr(u8)]
        enum InterruptUsageBits {
            Periodic,
            Notification,
            Rsvd2,
            Rsvd3,
        }

        if self.is_interrupt() {
            self.interrupt_usage_bits() == InterruptUsageBits::Periodic as u8
        } else {
            self.is_isoch()
        }
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
            if self.is_superspeedplus() { return 0 }
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
    pub fn has_ssp_companion(&self) -> bool {
        self.ssc.map(|ssc| ssc.attributes & (1 << 7) != 0).unwrap_or(false)
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
pub struct SuperSpeedPlusIsochCmp {
    pub kind: u8,
    pub bytes_per_interval: u32,
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
pub struct PortReq {
    pub direction: PortReqDirection,
    pub req_type: PortReqTy,
    pub req_recipient: PortReqRecipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    pub transfers_data: bool,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum PortReqDirection {
    HostToDevice,
    DeviceToHost,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PortReqTy {
    Class,
    Vendor,
    Standard,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PortReqRecipient {
    Device,
    Interface,
    Endpoint,
    Other,
    VendorSpecific,
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
pub struct Invalid(pub &'static str);

impl str::FromStr for PortState {
    type Err = Invalid;

    fn from_str(s: &str) -> result::Result<Self, Self::Err> {
        Ok(match s {
            "enabled_or_disabled" | "enabled/disabled" => Self::EnabledOrDisabled,
            "default" => Self::Default,
            "addressed" => Self::Addressed,
            "configured" => Self::Configured,
            _ => return Err(Invalid("read reserved port state")),
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum EndpointStatus {
    Disabled,
    Enabled,
    Halted,
    Stopped,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum PortTransferStatus {
    Success,
    ShortPacket(u16),
    Stalled,
    Unknown,
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
            _ => return Err(Invalid("read reserved endpoint state")),
        })
    }
}

pub enum DeviceReqData<'a> {
    In(&'a mut [u8]),
    Out(&'a [u8]),
    NoData,
}
impl DeviceReqData<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::In(buf) => buf.len(),
            Self::Out(buf) => buf.len(),
            Self::NoData => 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn map_buf<T, F: Fn(&[u8]) -> T>(&self, f: F) -> Option<T> {
        match self {
            Self::In(sbuf) => Some(f(sbuf)),
            Self::Out(dbuf) => Some(f(dbuf)),
            _ => None,
        }
    }
    pub fn direction(&self) -> PortReqDirection {
        match self {
            DeviceReqData::Out(_) => PortReqDirection::HostToDevice,
            DeviceReqData::NoData => PortReqDirection::HostToDevice,
            DeviceReqData::In(_) => PortReqDirection::DeviceToHost,
        }
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
        let path = format!("{}:port{}/configure", self.scheme, self.port);
        let json = serde_json::to_vec(req)?;
        let mut file = OpenOptions::new().read(false).write(true).open(path)?;
        let json_bytes_written = file.write(&json)?;
        if json_bytes_written != json.len() {
            return Err(XhciClientHandleError::InvalidResponse(Invalid(
                "configure_endpoints didn't read as many bytes as were requested",
            )));
        }
        Ok(())
    }
    pub fn port_state(&self) -> result::Result<PortState, XhciClientHandleError> {
        let path = format!("{}:port{}/state", self.scheme, self.port);
        let string = std::fs::read_to_string(path)?;
        Ok(string.parse()?)
    }
    pub fn open_endpoint_ctl(
        &self,
        num: u8,
    ) -> result::Result<File, XhciClientHandleError> {
        let path = format!("{}:port{}/endpoints/{}/ctl", self.scheme, self.port, num);
        Ok(File::open(path)?)
    }
    pub fn open_endpoint_data(
        &self,
        num: u8,
    ) -> result::Result<File, XhciClientHandleError> {
        let path = format!(
            "{}:port{}/endpoints/{}/data",
            self.scheme, self.port, num
        );
        Ok(File::open(path)?)
    }
    pub fn open_endpoint(&self, num: u8) -> result::Result<XhciEndpHandle, XhciClientHandleError> {
        Ok(XhciEndpHandle {
            ctl: self.open_endpoint_ctl(num)?,
            data: self.open_endpoint_data(num)?,
        })
    }
    pub fn device_request<'a>(
        &self,
        req_type: PortReqTy,
        req_recipient: PortReqRecipient,
        request: u8,
        value: u16,
        index: u16,
        data: DeviceReqData<'a>,
    ) -> result::Result<(), XhciClientHandleError> {
        let length = u16::try_from(data.len())
            .or(Err(XhciClientHandleError::TransferBufTooLarge(data.len())))?;

        let req = PortReq {
            direction: data.direction(),
            req_type,
            req_recipient,
            request,
            value,
            index,
            length,
            transfers_data: true,
        };
        let json = serde_json::to_vec(&req)?;

        let path = format!("{}:port{}/request", self.scheme, self.port);
        let mut file = File::open(path)?;

        let json_bytes_written = file.write(&json)?;
        if json_bytes_written != json.len() {
            return Err(XhciClientHandleError::InvalidResponse(Invalid(
                "device_request didn't return the same number of bytes as were written",
            )));
        }

        match data {
            DeviceReqData::In(buf) => {
                let bytes_read = file.read(buf)?;

                if bytes_read != buf.len() {
                    return Err(XhciClientHandleError::InvalidResponse(Invalid(
                        "device_request didn't transfer (host2dev) all bytes",
                    )));
                }
            }
            DeviceReqData::Out(buf) => {
                let bytes_read = file.write(&buf)?;

                if bytes_read != buf.len() {
                    return Err(XhciClientHandleError::InvalidResponse(Invalid(
                        "device_request didn't transfer (dev2host) all bytes",
                    )));
                }
            }
            DeviceReqData::NoData => (),
        }
        Ok(())
    }
    pub fn get_descriptor(
        &self,
        recipient: PortReqRecipient,
        ty: u8,
        idx: u8,
        windex: u16,
        buffer: &mut [u8],
    ) -> result::Result<(), XhciClientHandleError> {
        self.device_request(
            PortReqTy::Standard,
            recipient,
            0x06,
            (u16::from(ty) << 8) | u16::from(idx),
            windex,
            DeviceReqData::In(buffer),
        )
    }
    pub fn clear_feature(
        &self,
        recipient: PortReqRecipient,
        index: u16,
        feature_sel: u16,
    ) -> result::Result<(), XhciClientHandleError> {
        self.device_request(
            PortReqTy::Standard,
            recipient,
            0x01,
            feature_sel,
            index,
            DeviceReqData::NoData,
        )
    }
}

#[derive(Debug)]
pub struct XhciEndpHandle {
    data: File,
    ctl: File,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum XhciEndpCtlDirection {
    Out,
    In,
    NoData,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum XhciEndpCtlReq {
    // TODO: Reduce the number of direction enums from 5 to perhaps 2.
    // TODO: Allow to send multiple buffers in one transfer.
    /// Tells xhcid that a buffer is about to be sent from the Data interface file, to the
    /// endpoint.
    Transfer(XhciEndpCtlDirection),
    // TODO: Allow clients to specify what to reset.

    /// Tells xhcid that the endpoint is going to be reset.
    Reset {
        /// Only issue the Reset Endpoint and Set TR Dequeue Pointer commands, and let the client
        /// itself send a potential ClearFeature(ENDPOINT_HALT).
        no_clear_feature: bool,
    },

    /// Tells xhcid that the endpoint status is going to be retrieved from the Ctl interface file.
    Status,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum XhciEndpCtlRes {
    /// Xhcid responded with the current state of an endpoint.
    Status(EndpointStatus),

    /// Xhci sent the result of a transfer.
    TransferResult(PortTransferStatus),

    /// Xhcid is waiting for data to be sent or received on the Data interface file.
    Pending,

    /// No Ctl request is currently being processed by xhcid.
    Idle,
}

impl XhciEndpHandle {
    fn ctl_req(&mut self, ctl_req: &XhciEndpCtlReq) -> result::Result<(), XhciClientHandleError> {
        let ctl_buffer = serde_json::to_vec(ctl_req)?;

        let ctl_bytes_written = self.ctl.write(&ctl_buffer)?;
        if ctl_bytes_written != ctl_buffer.len() {
            return Err(Invalid("xhcid didn't process all of the ctl bytes").into());
        }

        Ok(())
    }
    fn ctl_res(&mut self) -> result::Result<XhciEndpCtlRes, XhciClientHandleError> {
        // a response must never exceed 256 bytes
        let mut ctl_buffer = [0u8; 256];
        let ctl_bytes_read = self.ctl.read(&mut ctl_buffer)?;

        let ctl_res = serde_json::from_slice(&ctl_buffer[..ctl_bytes_read as usize])?;
        Ok(ctl_res)
    }
    pub fn reset(&mut self, no_clear_feature: bool) -> result::Result<(), XhciClientHandleError> {
        self.ctl_req(&XhciEndpCtlReq::Reset { no_clear_feature })
    }
    pub fn status(&mut self) -> result::Result<EndpointStatus, XhciClientHandleError> {
        self.ctl_req(&XhciEndpCtlReq::Status)?;
        match self.ctl_res()? {
            XhciEndpCtlRes::Status(s) => Ok(s),
            _ => Err(Invalid("expected status response").into()),
        }
    }
    fn generic_transfer<F: FnOnce(&mut File) -> io::Result<usize>>(&mut self, direction: XhciEndpCtlDirection, f: F, expected_len: usize) -> result::Result<PortTransferStatus, XhciClientHandleError> {
        let req = XhciEndpCtlReq::Transfer(direction);
        self.ctl_req(&req)?;

        let bytes_read = f(&mut self.data)?;
        let res = self.ctl_res()?;

        match res {
            XhciEndpCtlRes::TransferResult(PortTransferStatus::Success) if bytes_read != expected_len => Err(Invalid("no short packet, but fewer bytes were read/written").into()),
            XhciEndpCtlRes::TransferResult(r) => Ok(r),
            _ => Err(Invalid("expected transfer result").into()),
        }
    }
    pub fn transfer_write(&mut self, buf: &[u8]) -> result::Result<PortTransferStatus, XhciClientHandleError> {
        self.generic_transfer(XhciEndpCtlDirection::Out, |data| data.write(buf), buf.len())
    }
    pub fn transfer_read(&mut self, buf: &mut [u8]) -> result::Result<PortTransferStatus, XhciClientHandleError> {
        let len = buf.len();
        self.generic_transfer(XhciEndpCtlDirection::In, |data| data.read(buf), len)
    }
    pub fn transfer_nodata(&mut self, buf: &[u8]) -> result::Result<PortTransferStatus, XhciClientHandleError> {
        self.generic_transfer(XhciEndpCtlDirection::NoData, |_| Ok(0), buf.len())
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

    #[error("transfer buffer too large ({0} > 65536)")]
    TransferBufTooLarge(usize),
}
