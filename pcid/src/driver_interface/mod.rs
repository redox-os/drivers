use std::fmt;
use std::fs::File;
use std::io::prelude::*;
use std::ptr::NonNull;
use std::{env, io};

use log::info;
use std::os::unix::io::{FromRawFd, RawFd};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

pub use bar::PciBar;
pub use cap::VendorSpecificCapability;
pub use id::FullDeviceId;
pub use pci_types::PciAddress;

mod bar;
pub mod cap;
mod id;
pub mod irq_helpers;
pub mod msi;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LegacyInterruptLine {
    #[doc(hidden)]
    pub irq: u8,
    pub phandled: Option<(u32, [u32; 3])>,
}

impl LegacyInterruptLine {
    /// Get an IRQ handle for this interrupt line.
    pub fn irq_handle(self, driver: &str) -> File {
        if let Some((phandle, addr)) = self.phandled {
            File::create(format!(
                "/scheme/irq/phandle-{}/{},{},{}",
                phandle, addr[0], addr[1], addr[2]
            ))
            .unwrap_or_else(|err| panic!("{driver}: failed to open IRQ file: {err}"))
        } else {
            File::open(format!("/scheme/irq/{}", self.irq))
                .unwrap_or_else(|err| panic!("{driver}: failed to open IRQ file: {err}"))
        }
    }
}

impl fmt::Display for LegacyInterruptLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((phandle, addr)) = self.phandled {
            write!(f, "(phandle {}, {:?})", phandle, addr)
        } else {
            write!(f, "{}", self.irq)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PciAddress")]
struct PciAddressDef {
    #[serde(getter = "PciAddress::segment")]
    segment: u16,
    #[serde(getter = "PciAddress::bus")]
    bus: u8,
    #[serde(getter = "PciAddress::device")]
    device: u8,
    #[serde(getter = "PciAddress::function")]
    function: u8,
}

impl From<PciAddressDef> for PciAddress {
    fn from(value: PciAddressDef) -> Self {
        PciAddress::new(value.segment, value.bus, value.device, value.function)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PciFunction {
    /// Address of the PCI function.
    #[serde(with = "PciAddressDef")]
    pub addr: PciAddress,

    /// PCI Base Address Registers
    pub bars: [PciBar; 6],

    /// Legacy IRQ line: It's the responsibility of pcid to make sure that it be mapped in either
    /// the I/O APIC or the 8259 PIC, so that the subdriver can map the interrupt vector directly.
    /// The vector to map is always this field, plus 32.
    /// If INTx# interrupts aren't supported at all this is `None`.
    pub legacy_interrupt_line: Option<LegacyInterruptLine>,

    /// All identifying information of the PCI function.
    pub full_device_id: FullDeviceId,
}
impl PciFunction {
    pub fn name(&self) -> String {
        // FIXME stop replacing : with - once it is a valid character in scheme names
        format!("pci-{}", self.addr).replace(':', "-")
    }

    pub fn display(&self) -> String {
        let mut string = self.name();
        let mut first = true;
        for (i, bar) in self.bars.iter().enumerate() {
            if !bar.is_none() {
                if first {
                    first = false;
                    string.push_str(" on:");
                }
                string.push_str(&format!(" {i}={}", bar.display()));
            }
        }
        if let Some(irq) = self.legacy_interrupt_line {
            string.push_str(&format!(" IRQ: {irq}"));
        }
        string
    }
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
        if let &Self::Enabled = self {
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum PciFeature {
    Msi,
    MsiX,
}
impl PciFeature {
    pub fn is_msi(self) -> bool {
        if let Self::Msi = self {
            true
        } else {
            false
        }
    }
    pub fn is_msix(self) -> bool {
        if let Self::MsiX = self {
            true
        } else {
            false
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum PciFeatureInfo {
    Msi(msi::MsiInfo),
    MsiX(msi::MsixInfo),
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

// TODO: Remove these "features" and just go strait to the actual thing.

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MsiSetFeatureInfo {
    /// The Multi Message Enable field of the Message Control in the MSI Capability Structure,
    /// is the log2 of the interrupt vectors, minus one. Can only be 0b000..=0b101.
    pub multi_message_enable: Option<u8>,

    /// The system-specific message address and data.
    ///
    /// The message address contains things like the CPU that will be targeted, at least on
    /// x86_64. The message data contains the actual interrupt vector (lower 8 bits) and
    /// the kind of interrupt, at least on x86_64.
    pub message_address_and_data: Option<msi::MsiAddrAndData>,

    /// A bitmap of the vectors that are masked. This field is not guaranteed (and not likely,
    /// at least according to the feature flags I got from QEMU), to exist.
    pub mask_bits: Option<u32>,
}

/// Some flags that might be set simultaneously, but separately.
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SetFeatureInfo {
    Msi(MsiSetFeatureInfo),

    MsiX {
        /// Masks the entire function, and all of its vectors.
        function_mask: Option<bool>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientRequest {
    RequestConfig,
    RequestFeatures,
    RequestVendorCapabilities,
    EnableFeature(PciFeature),
    FeatureInfo(PciFeature),
    SetFeatureInfo(SetFeatureInfo),
    ReadConfig(u16),
    WriteConfig(u16, u32),
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidServerResponseError {
    NonexistentFeature(PciFeature),
    InvalidBitPattern,
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientResponse {
    Config(SubdriverArguments),
    AllFeatures(Vec<PciFeature>),
    VendorCapabilities(Vec<VendorSpecificCapability>),
    FeatureEnabled(PciFeature),
    FeatureStatus(PciFeature, FeatureStatus),
    Error(PcidServerResponseError),
    FeatureInfo(PciFeature, PciFeatureInfo),
    SetFeatureInfo(PciFeature),
    ReadConfig(u32),
    WriteConfig,
}

pub struct MappedBar {
    pub ptr: NonNull<u8>,
    pub bar_size: usize,
}

// TODO: Ideally, pcid might have its own scheme, like lots of other Redox drivers, where this kind of IPC is done. Otherwise, instead of writing serde messages over
// a channel, the communication could potentially be done via mmap, using a channel
// very similar to crossbeam-channel or libstd's mpsc (except the cycle, enqueue and dequeue fields
// are stored in the same buffer as the actual data).
/// A handle from a `pcid` client (e.g. `ahcid`) to `pcid`.
pub struct PciFunctionHandle {
    pcid_to_client: File,
    pcid_from_client: File,
    config: SubdriverArguments,
    mapped_bars: [Option<MappedBar>; 6],
}

#[doc(hidden)]
pub fn send<W: Write, T: Serialize>(w: &mut W, message: &T) -> Result<()> {
    let mut data = Vec::new();
    bincode::serialize_into(&mut data, message)?;
    let length_bytes = u64::to_le_bytes(data.len() as u64);
    w.write_all(&length_bytes)?;
    w.write_all(&data)?;
    Ok(())
}
#[doc(hidden)]
pub fn recv<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut length_bytes = [0u8; 8];
    r.read_exact(&mut length_bytes)?;
    let length = u64::from_le_bytes(length_bytes);
    if length > 0x100_000 {
        panic!("pcid_interface: buffer too large");
    }
    let mut data = vec![0u8; length as usize];
    r.read_exact(&mut data)?;

    Ok(bincode::deserialize_from(&data[..])?)
}

impl PciFunctionHandle {
    pub fn connect_default() -> Result<Self> {
        let pcid_to_client_fd = env::var("PCID_TO_CLIENT_FD")?
            .parse::<RawFd>()
            .map_err(PcidClientHandleError::EnvValidityError)?;
        let pcid_from_client_fd = env::var("PCID_FROM_CLIENT_FD")?
            .parse::<RawFd>()
            .map_err(PcidClientHandleError::EnvValidityError)?;

        let mut pcid_to_client = unsafe { File::from_raw_fd(pcid_to_client_fd) };
        let mut pcid_from_client = unsafe { File::from_raw_fd(pcid_from_client_fd) };

        send(&mut pcid_from_client, &PcidClientRequest::RequestConfig)?;
        let config = match recv(&mut pcid_to_client)? {
            PcidClientResponse::Config(a) => a,
            other => return Err(PcidClientHandleError::InvalidResponse(other)),
        };

        Ok(Self {
            pcid_to_client,
            pcid_from_client,
            config,
            mapped_bars: [const { None }; 6],
        })
    }
    fn send(&mut self, req: &PcidClientRequest) -> Result<()> {
        send(&mut self.pcid_from_client, req)
    }
    fn recv(&mut self) -> Result<PcidClientResponse> {
        recv(&mut self.pcid_to_client)
    }
    pub fn config(&self) -> SubdriverArguments {
        self.config.clone()
    }

    pub fn get_vendor_capabilities(&mut self) -> Result<Vec<VendorSpecificCapability>> {
        self.send(&PcidClientRequest::RequestVendorCapabilities)?;

        match self.recv()? {
            PcidClientResponse::VendorCapabilities(a) => Ok(a),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }

    // FIXME turn into struct with bool fields
    pub fn fetch_all_features(&mut self) -> Result<Vec<PciFeature>> {
        self.send(&PcidClientRequest::RequestFeatures)?;
        match self.recv()? {
            PcidClientResponse::AllFeatures(a) => Ok(a),
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
    pub fn set_feature_info(&mut self, info: SetFeatureInfo) -> Result<()> {
        self.send(&PcidClientRequest::SetFeatureInfo(info))?;
        match self.recv()? {
            PcidClientResponse::SetFeatureInfo(_) => Ok(()),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub unsafe fn read_config(&mut self, offset: u16) -> Result<u32> {
        self.send(&PcidClientRequest::ReadConfig(offset))?;
        match self.recv()? {
            PcidClientResponse::ReadConfig(value) => Ok(value),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub unsafe fn write_config(&mut self, offset: u16, value: u32) -> Result<()> {
        self.send(&PcidClientRequest::WriteConfig(offset, value))?;
        match self.recv()? {
            PcidClientResponse::WriteConfig => Ok(()),
            other => Err(PcidClientHandleError::InvalidResponse(other)),
        }
    }
    pub unsafe fn map_bar(&mut self, bir: u8) -> Result<&MappedBar> {
        let mapped_bar = &mut self.mapped_bars[bir as usize];
        if let Some(mapped_bar) = mapped_bar {
            Ok(mapped_bar)
        } else {
            let (bar, bar_size) = self.config.func.bars[bir as usize].expect_mem();

            let ptr = unsafe {
                common::physmap(
                    bar,
                    bar_size,
                    common::Prot::RW,
                    // FIXME once the kernel supports this use write-through for prefetchable BAR
                    common::MemoryType::Uncacheable,
                )
            }
            .map_err(|err| io::Error::other(format!("failed to map BAR at {bar:016X}: {err}")))?;

            Ok(mapped_bar.insert(MappedBar {
                ptr: NonNull::new(ptr.cast::<u8>()).expect("Mapping a BAR resulted in a nullptr"),
                bar_size,
            }))
        }
    }
}
