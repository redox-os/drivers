use std::fs::{File, OpenOptions};
use std::io::prelude::*;
use std::{env, io};

use std::os::unix::io::{FromRawFd, RawFd};

use serde::{Serialize, Deserialize, de::DeserializeOwned};
use thiserror::Error;

pub use crate::pci::PciBar;
pub use crate::pci::msi::{MsiCapability, MsixCapability, MsixTableEntry};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PciFunction {
    /// Number of PCI bus
    pub bus_num: u8,
    /// Number of PCI device
    pub dev_num: u8,
    /// Number of PCI function
    pub func_num: u8,
    /// PCI Base Address Registers
    pub bars: [PciBar; 6],
    /// BAR sizes
    pub bar_sizes: [u32; 6],
    /// Legacy IRQ line
    // TODO: Stop using legacy IRQ lines, and physical pins, but MSI/MSI-X instead.
    pub legacy_interrupt_line: u8,
    /// Vendor ID
    pub venid: u16,
    /// Device ID
    pub devid: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubdriverArguments {
    pub func: PciFunction,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum FeatureStatus {
    Enabled,
    Disabled,
}

impl FeatureStatus {
    pub fn enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
    pub fn is_enabled(&self) -> bool {
        if let &Self::Enabled = self { true } else { false }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum PciFeature {
    Msi,
    MsiX,
}
impl PciFeature {
    pub fn is_msi(&self) -> bool {
        if let &Self::Msi = self { true } else { false }
    }
    pub fn is_msix(&self) -> bool {
        if let &Self::MsiX = self { true } else { false }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum PciFeatureInfo {
    Msi(MsiCapability),
    MsiX(MsixCapability),
}

#[derive(Debug, Error)]
pub enum PcidClientHandleError {
    #[error("i/o error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON ser/de error: {0}")]
    SerializationError(#[from] bincode::Error),

    #[error("environment variable error: {0}")]
    EnvError(#[from] env::VarError),

    #[error("malformed fd: {0}")]
    EnvValidityError(std::num::ParseIntError),

    #[error("invalid response: {0:?}")]
    InvalidResponse(PcidClientResponse),
}
pub type Result<T, E = PcidClientHandleError> = std::result::Result<T, E>;

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientRequest {
    RequestConfig,
    RequestFeatures,
    EnableFeature(PciFeature),
    FeatureStatus(PciFeature),
    FeatureInfo(PciFeature),
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidServerResponseError {
    NonexistentFeature(PciFeature),
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientResponse {
    Config(SubdriverArguments),
    AllFeatures(Vec<(PciFeature, FeatureStatus)>),
    FeatureEnabled(PciFeature),
    FeatureStatus(PciFeature, FeatureStatus),
    Error(PcidServerResponseError),
    FeatureInfo(PciFeature, PciFeatureInfo),
}

// TODO: Ideally, pcid might have its own scheme, like lots of other Redox drivers, where this kind of IPC is done. Otherwise, instead of writing serde messages over
// a channel, the communication could potentially be done via mmap, using a channel
// very similar to crossbeam-channel or libstd's mpsc (except the cycle, enqueue and dequeue fields
// are stored in the same buffer as the actual data).
/// A handle from a `pcid` client (e.g. `ahcid`) to `pcid`.
pub struct PcidServerHandle {
    pcid_to_client: File,
    pcid_from_client: File,
}

pub(crate) fn send<W: Write, T: Serialize>(w: &mut W, message: &T) -> Result<()> {
    let mut data = Vec::new();
    bincode::serialize_into(&mut data, message)?;
    let length_bytes = u64::to_le_bytes(data.len() as u64);
    w.write_all(&length_bytes)?;
    w.write_all(&data)?;
    Ok(())
}
pub(crate) fn recv<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut length_bytes = [0u8; 8];
    r.read_exact(&mut length_bytes)?;
    let length = u64::from_le_bytes(length_bytes);
    if length > 0x100_000 {
        panic!("pcid_interface: buffer too large");
    }
    let mut data = vec! [0u8; length as usize];
    r.read_exact(&mut data)?;

    Ok(bincode::deserialize_from(&data[..])?)
}

impl PcidServerHandle {
    pub fn connect(pcid_to_client: RawFd, pcid_from_client: RawFd) -> Result<Self> {
        Ok(Self {
            pcid_to_client: unsafe { File::from_raw_fd(pcid_to_client) },
            pcid_from_client: unsafe { File::from_raw_fd(pcid_from_client) },
        })
    }
    pub fn connect_default() -> Result<Self> {
        let pcid_to_client_fd = env::var("PCID_TO_CLIENT_FD")?.parse::<RawFd>().map_err(PcidClientHandleError::EnvValidityError)?;
        let pcid_from_client_fd = env::var("PCID_FROM_CLIENT_FD")?.parse::<RawFd>().map_err(PcidClientHandleError::EnvValidityError)?;

        Self::connect(pcid_to_client_fd, pcid_from_client_fd)
    }
    pub(crate) fn send(&mut self, req: &PcidClientRequest) -> Result<()> {
        send(&mut self.pcid_from_client, req)
    }
    pub(crate) fn recv(&mut self) -> Result<PcidClientResponse> {
        recv(&mut self.pcid_to_client)
    }
    pub fn fetch_config(&mut self) -> Result<SubdriverArguments> {
        self.send(&PcidClientRequest::RequestConfig)?;
        match self.recv()? {
            PcidClientResponse::Config(a) => Ok(a),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub fn fetch_all_features(&mut self) -> Result<Vec<(PciFeature, FeatureStatus)>> {
        self.send(&PcidClientRequest::RequestFeatures)?;
        match self.recv()? {
            PcidClientResponse::AllFeatures(a) => Ok(a),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub fn feature_status(&mut self, feature: PciFeature) -> Result<FeatureStatus> {
        self.send(&PcidClientRequest::FeatureStatus(feature))?;
        match self.recv()? {
            PcidClientResponse::FeatureStatus(feat, status) if feat == feature => Ok(status),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub fn enable_feature(&mut self, feature: PciFeature) -> Result<()> {
        self.send(&PcidClientRequest::EnableFeature(feature))?;
        match self.recv()? {
            PcidClientResponse::FeatureEnabled(feat) if feat == feature => Ok(()),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub fn feature_info(&mut self, feature: PciFeature) -> Result<PciFeatureInfo> {
        self.send(&PcidClientRequest::FeatureInfo(feature))?;
        match self.recv()? {
            PcidClientResponse::FeatureInfo(feat, info) if feat == feature => Ok(info),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
}
