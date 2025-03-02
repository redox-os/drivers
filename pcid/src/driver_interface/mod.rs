use std::fs::File;
use std::io::prelude::*;
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
use std::path::Path;
use std::ptr::NonNull;
use std::{env, io};
use std::{fmt, process};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub use bar::PciBar;
pub use cap::VendorSpecificCapability;
pub use id::FullDeviceId;
pub use pci_types::PciAddress;

mod bar;
pub mod cap;
pub mod config;
mod id;
pub mod irq_helpers;
pub mod msi;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LegacyInterruptLine {
    #[doc(hidden)]
    pub irq: u8,
    pub phandled: Option<(u32, [u32; 3], usize)>,
}

impl LegacyInterruptLine {
    /// Get an IRQ handle for this interrupt line.
    pub fn irq_handle(self, driver: &str) -> File {
        if let Some((phandle, addr, cells)) = self.phandled {
            let path = match cells {
                1 => format!("/scheme/irq/phandle-{}/{}", phandle, addr[0]),
                2 => format!("/scheme/irq/phandle-{}/{},{}", phandle, addr[0], addr[1]),
                3 => format!(
                    "/scheme/irq/phandle-{}/{},{},{}",
                    phandle, addr[0], addr[1], addr[2]
                ),
                _ => panic!(
                    "unexpected number of IRQ description cells for phandle {phandle}: {cells}"
                ),
            };
            File::create(path)
                .unwrap_or_else(|err| panic!("{driver}: failed to open IRQ file: {err}"))
        } else {
            File::open(format!("/scheme/irq/{}", self.irq))
                .unwrap_or_else(|err| panic!("{driver}: failed to open IRQ file: {err}"))
        }
    }
}

impl fmt::Display for LegacyInterruptLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((phandle, addr, cells)) = self.phandled {
            match cells {
                1 => write!(f, "(phandle {}, {:?})", phandle, addr[0]),
                2 => write!(f, "(phandle {}, {:?},{:?})", phandle, addr[0], addr[1]),
                3 => write!(f, "(phandle {}, {:?})", phandle, addr),
                _ => panic!(
                    "unexpected number of IRQ description cells for phandle {phandle}: {cells}"
                ),
            }
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
    EnableDevice,
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
    EnabledDevice,
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

/// A handle from a `pcid` client (e.g. `ahcid`) to `pcid`.
pub struct PciFunctionHandle {
    channel: File,
    config: SubdriverArguments,
    mapped_bars: [Option<MappedBar>; 6],
}

fn send<T: Serialize>(w: &mut File, message: &T) {
    let mut data = Vec::new();
    bincode::serialize_into(&mut data, message).expect("couldn't serialize pcid message");
    match w.write(&data) {
        Ok(len) => assert_eq!(len, data.len()),
        Err(err) => {
            log::error!("writing pcid request failed: {err}");
            process::exit(1);
        }
    }
}
fn recv<T: DeserializeOwned>(r: &mut File) -> T {
    let mut length_bytes = [0u8; 8];
    if let Err(err) = r.read_exact(&mut length_bytes) {
        log::error!("reading pcid response length failed: {err}");
        process::exit(1);
    }
    let length = u64::from_le_bytes(length_bytes);
    if length > 0x100_000 {
        panic!("pcid_interface: buffer too large");
    }
    let mut data = vec![0u8; length as usize];
    if let Err(err) = r.read_exact(&mut data) {
        log::error!("reading pcid response failed: {err}");
        process::exit(1);
    }

    bincode::deserialize_from(&data[..]).expect("couldn't deserialize pcid message")
}

impl PciFunctionHandle {
    pub fn connect_default() -> Self {
        let channel_fd = match env::var("PCID_CLIENT_CHANNEL") {
            Ok(channel_fd) => channel_fd,
            Err(err) => {
                log::error!("PCID_CLIENT_CHANNEL invalid: {err}");
                process::exit(1);
            }
        };
        let channel_fd = match channel_fd.parse::<RawFd>() {
            Ok(channel_fd) => channel_fd,
            Err(err) => {
                log::error!("PCID_CLIENT_CHANNEL invalid: {err}");
                process::exit(1);
            }
        };
        Self::connect_common(channel_fd)
    }

    pub fn connect_by_path(device_path: &Path) -> io::Result<Self> {
        let channel_fd = syscall::open(
            device_path.join("channel").to_str().unwrap(),
            syscall::O_RDWR,
        )
        .map_err(|err| io::Error::other(format!("failed to open pcid channel: {}", err)))?;
        Ok(Self::connect_common(channel_fd as RawFd))
    }

    fn connect_common(channel_fd: i32) -> PciFunctionHandle {
        let mut channel = unsafe { File::from_raw_fd(channel_fd) };

        send(&mut channel, &PcidClientRequest::RequestConfig);
        let config = match recv(&mut channel) {
            PcidClientResponse::Config(a) => a,
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        };

        Self {
            channel,
            config,
            mapped_bars: [const { None }; 6],
        }
    }

    pub fn into_inner_fd(self) -> RawFd {
        self.channel.into_raw_fd()
    }

    fn send(&mut self, req: &PcidClientRequest) {
        send(&mut self.channel, req)
    }
    fn recv(&mut self) -> PcidClientResponse {
        recv(&mut self.channel)
    }

    pub fn config(&self) -> SubdriverArguments {
        self.config.clone()
    }

    pub fn enable_device(&mut self) {
        self.send(&PcidClientRequest::EnableDevice);
        match self.recv() {
            PcidClientResponse::EnabledDevice => {}
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }

    pub fn get_vendor_capabilities(&mut self) -> Vec<VendorSpecificCapability> {
        self.send(&PcidClientRequest::RequestVendorCapabilities);
        match self.recv() {
            PcidClientResponse::VendorCapabilities(a) => a,
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }

    // FIXME turn into struct with bool fields
    pub fn fetch_all_features(&mut self) -> Vec<PciFeature> {
        self.send(&PcidClientRequest::RequestFeatures);
        match self.recv() {
            PcidClientResponse::AllFeatures(a) => a,
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub fn enable_feature(&mut self, feature: PciFeature) {
        self.send(&PcidClientRequest::EnableFeature(feature));
        match self.recv() {
            PcidClientResponse::FeatureEnabled(feat) if feat == feature => {}
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub fn feature_info(&mut self, feature: PciFeature) -> PciFeatureInfo {
        self.send(&PcidClientRequest::FeatureInfo(feature));
        match self.recv() {
            PcidClientResponse::FeatureInfo(feat, info) if feat == feature => info,
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub fn set_feature_info(&mut self, info: SetFeatureInfo) {
        self.send(&PcidClientRequest::SetFeatureInfo(info));
        match self.recv() {
            PcidClientResponse::SetFeatureInfo(_) => {}
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub unsafe fn read_config(&mut self, offset: u16) -> u32 {
        self.send(&PcidClientRequest::ReadConfig(offset));
        match self.recv() {
            PcidClientResponse::ReadConfig(value) => value,
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub unsafe fn write_config(&mut self, offset: u16, value: u32) {
        self.send(&PcidClientRequest::WriteConfig(offset, value));
        match self.recv() {
            PcidClientResponse::WriteConfig => {}
            other => {
                log::error!("received wrong pcid response: {other:?}");
                process::exit(1);
            }
        }
    }
    pub unsafe fn map_bar(&mut self, bir: u8) -> &MappedBar {
        let mapped_bar = &mut self.mapped_bars[bir as usize];
        if let Some(mapped_bar) = mapped_bar {
            mapped_bar
        } else {
            let (bar, bar_size) = self.config.func.bars[bir as usize].expect_mem();

            let ptr = match unsafe {
                common::physmap(
                    bar,
                    bar_size,
                    common::Prot::RW,
                    // FIXME once the kernel supports this use write-through for prefetchable BAR
                    common::MemoryType::Uncacheable,
                )
            } {
                Ok(ptr) => ptr,
                Err(err) => {
                    log::error!("failed to map BAR at {bar:016X}: {err}");
                    process::exit(1);
                }
            };

            mapped_bar.insert(MappedBar {
                ptr: NonNull::new(ptr.cast::<u8>()).expect("Mapping a BAR resulted in a nullptr"),
                bar_size,
            })
        }
    }
}
