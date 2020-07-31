use std::convert::{TryFrom, TryInto};
use std::fs::File;
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};
use std::{env, fmt, io, mem};

use syscall::error::Error as Errno;
use syscall::error::{ENOMEM, EOVERFLOW};
use syscall::io_uring::v1::{Priority, SqEntry64};

use redox_iou::instance::ConsumerInstanceBuilder;
use redox_iou::memory::{BufferPool, BufferSlice};
use redox_iou::reactor::{Handle as IoringReactorHandle, SecondaryRingId};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

pub use crate::pci::{
    cap::{Capability as PciCapability, CapabilityRawTagged as PciCapabilityRawTagged},
    msi, PciBar,
};
pub use crate::pcie::cap::{
    Capability as PcieCapability, CapabilityRawTagged as PcieCapabilityRawTagged,
};

pub mod helpers;

/// A legacy INTx# pin, mapped to an interrupt through the 8259 PIC or the I/O APIC.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum LegacyInterruptPin {
    /// INTa#
    IntA = 1,
    /// INTb#
    IntB = 2,
    /// INTc#
    IntC = 3,
    /// INTd#
    IntD = 4,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct PciFunction {
    /// Number of PCI bus
    pub bus_num: u8,

    /// Number of PCI device
    pub dev_num: u8,

    /// Number of PCI function
    pub func_num: u8,

    /// PCI Base Address Registers
    pub bars: [Option<PciBar>; 6],

    /// BAR sizes
    pub bar_sizes: [u32; 6],

    /// Legacy IRQ line: It's the responsibility of pcid to make sure that it be mapped in either
    /// the I/O APIC or the 8259 PIC, so that the subdriver can map the interrupt vector directly.
    ///
    /// The vector to map is always this field, plus 32.
    pub legacy_interrupt_line: u8,

    /// Legacy interrupt pin (INTx#), none if INTx# interrupts aren't supported at all.
    ///
    /// This field must either be 0 for no INTx# IRQ support, or 1 to 4 for INTa# to INTd#,
    /// respectively.
    pub legacy_interrupt_pin: u8,

    /// Vendor ID
    pub venid: u16,

    /// Device ID
    pub devid: u16,
}
unsafe impl plain::Plain for PciFunction {}

impl PciFunction {
    pub fn name(&self) -> String {
        format!(
            "pci-{:>02X}.{:>02X}.{:>02X}",
            self.bus_num, self.dev_num, self.func_num
        )
    }
    pub fn legacy_interrupt_pin(&self) -> Option<LegacyInterruptPin> {
        match self.legacy_interrupt_pin {
            0 => None,
            1 => Some(LegacyInterruptPin::IntA),
            2 => Some(LegacyInterruptPin::IntB),
            3 => Some(LegacyInterruptPin::IntC),
            4 => Some(LegacyInterruptPin::IntD),
            _ => {
                log::warn!("Invalid interrupt pin number sent by pcid, returning None");
                None
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct SubdriverArguments {
    pub func: PciFunction,
}
unsafe impl plain::Plain for SubdriverArguments {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CapabilityType {
    Msi,
    MsiX,
    Pcie,
    PciPwrMgmt,
    Aer,

    // function specific
    Sata,
}
impl CapabilityType {
    pub fn is_msi(&self) -> bool {
        if let &Self::Msi = self {
            true
        } else {
            false
        }
    }
    pub fn is_msix(&self) -> bool {
        if let &Self::MsiX = self {
            true
        } else {
            false
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Capability {
    Pci(PciCapability),
    Pcie(PcieCapability),
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum CapabilityKind {
    Pci,
    Pcie,
}

#[derive(Clone, Copy)]
pub union CapabilityRaw {
    pub pci: PciCapabilityRawTagged,
    pub pcie: PcieCapabilityRawTagged,
}
#[derive(Clone, Copy)]
pub struct CapabilityRawTagged {
    pub kind: u8,
    pub raw: CapabilityRaw,
}
unsafe impl plain::Plain for CapabilityRawTagged {}

impl Capability {
    pub fn construct(kind: u8, raw: CapabilityRaw) -> Option<Self> {
        // unsafe due to accessing union fields
        unsafe {
            Some(if kind == CapabilityKind::Pci as u8 {
                Self::Pci(PciCapability::construct(raw.pci.id, raw.pci.raw)?)
            } else if kind == CapabilityKind::Pcie as u8 {
                Self::Pcie(PcieCapability::construct(
                    raw.pcie.id,
                    raw.pcie.version,
                    raw.pcie.raw,
                ))
            } else {
                return None;
            })
        }
    }
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Self::Pci(_) => CapabilityKind::Pci,
            Self::Pcie(_) => CapabilityKind::Pcie,
        }
    }
    pub fn as_pci_mut(&mut self) -> Option<&mut PciCapability> {
        match self {
            &mut Self::Pci(ref mut inner) => Some(inner),
            _ => None,
        }
    }
    pub fn as_pcie_mut(&mut self) -> Option<&mut PcieCapability> {
        match self {
            &mut Self::Pcie(ref mut inner) => Some(inner),
            _ => None,
        }
    }
    pub fn as_pci(&self) -> Option<&PciCapability> {
        match self {
            &Self::Pci(ref inner) => Some(inner),
            _ => None,
        }
    }
    pub fn as_pcie(&self) -> Option<&PcieCapability> {
        match self {
            &Self::Pcie(ref inner) => Some(inner),
            _ => None,
        }
    }
    pub fn into_pci(self) -> Option<PciCapability> {
        match self {
            Self::Pci(inner) => Some(inner),
            _ => None,
        }
    }
    pub fn into_pcie(self) -> Option<PcieCapability> {
        match self {
            Self::Pcie(inner) => Some(inner),
            _ => None,
        }
    }
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

    #[error("io_uring transport error: {0}")]
    IoUringTransportError(syscall::Error),
}

#[derive(Debug, Error)]
pub enum IoUringSetupError {
    #[error("io_uring instance creation error: {0}")]
    CreateInstanceError(syscall::Error),

    #[error("io_uring fmap error: {0}")]
    MapAllError(syscall::Error),

    #[error("io_uring attach error: {0}")]
    AttachError(syscall::Error),

    #[error("io_uring buffer error: {0}")]
    BufferError(syscall::Error),

    #[error("io_uring secondary instance error: {0}")]
    AddInstanceError(syscall::Error),
}

pub type Result<T, E = PcidClientHandleError> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct MsiSetCapabilityInfo {
    pub flags: u32,

    pub enabled: u8,

    /// The Multi Message Enable field of the Message Control in the MSI Capability Structure,
    /// is the log2 of the interrupt vectors, minus one. Can only be 0b000..=0b101.
    pub multi_message_enable: u8,

    /// The system-specific message address, must be DWORD aligned.
    ///
    /// The message address contains things like the CPU that will be targeted, at least on
    /// x86_64.
    pub message_address: u32,

    /// The upper 32 bits of the 64-bit message address. Not guaranteed to exist, and is
    /// reserved on x86_64 (currently).
    pub message_upper_address: u32,

    /// The message data, containing the actual interrupt vector (lower 8 bits), etc.
    ///
    /// The spec mentions that the lower N bits can be modified, where N is the multi message
    /// enable, which means that the vector set here has to be aligned to that number, and that
    /// all vectors in that range have to be allocated.
    pub message_data: u16,

    /// A bitmap of the vectors that are masked. This field is not guaranteed (and not likely,
    /// at least according to the feature flags I got from QEMU), to exist.
    pub mask_bits: u32,
}
bitflags::bitflags! {
    /// Tells what values will be modified.
    #[derive(Serialize, Deserialize)]
    pub struct MsiSetCapabilityInfoFlags: u32 {
        const ENABLED = 0x0000_0001;
        const MULTI_MESSAGE_ENABLE = 0x0000_0002;
        const MESSAGE_ADDRESS = 0x0000_0004;
        const MESSAGE_UPPER_ADDRESS = 0x0000_0008;
        const MESSAGE_DATA = 0x0000_0010;
        const MASK_BITS = 0x0000_0020;
    }
}
impl Default for MsiSetCapabilityInfoFlags {
    fn default() -> Self {
        Self::empty()
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct MsiXSetCapabilityInfo {
    pub flags: u32,

    pub enabled: u8,

    /// Masks the entire function, and all of its vectors.
    pub function_mask: u8,
}
bitflags::bitflags! {
    #[derive(Serialize, Deserialize)]
    pub struct MsiXSetCapabilityInfoFlags: u32 {
        const ENABLED = 0x0000_0001;
        const FUNCTION_MASK = 0x0000_0002;
    }
}
impl Default for MsiXSetCapabilityInfoFlags {
    fn default() -> Self {
        Self::empty()
    }
}

/// Some flags that might be set simultaneously, but separately.
#[derive(Clone, Copy)]
#[repr(C)]
struct SetCapabilityInfoRaw {
    id: u32,
    inner: SetCapabilityInfoInner,
}
unsafe impl plain::Plain for SetCapabilityInfoRaw {}

impl fmt::Debug for SetCapabilityInfoRaw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.id == CapabilityType::Msi as u32 {
            f.debug_tuple("SetCapabilityInfo::Msi")
                .field(unsafe { &self.inner.msi })
                .finish()
        } else if self.id == CapabilityType::MsiX as u32 {
            f.debug_tuple("SetCapabilityInfo::MsiX")
                .field(unsafe { &self.inner.msix })
                .finish()
        } else {
            write!(f, "SetCapabilityInfo::<unknown>")
        }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
union SetCapabilityInfoInner {
    msi: MsiSetCapabilityInfo,
    msix: MsiXSetCapabilityInfo,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SetCapabilityInfo {
    Msi(MsiSetCapabilityInfo),
    MsiX(MsiXSetCapabilityInfo),
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientRequest {
    RequestConfig,
    GetCapabilities,
    GetCapability(CapabilityType),
    SetCapability(SetCapabilityInfo),
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidServerResponseError {
    NonexistentCapability(CapabilityType),
    InvalidBitPattern,
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PcidClientResponse {
    Config(SubdriverArguments),
    AllCapabilities(Vec<Capability>),
    Capability(Option<Capability>),
    SetCapability,
    Error(PcidServerResponseError),
}

/// A handle from a `pcid` client (e.g. `ahcid`) to `pcid`.
pub struct PcidServerHandle {
    inner: PcidServerTransport,
}

enum PcidServerTransport {
    Pipe {
        pcid_to_client: File,
        pcid_from_client: File,
    },
    IoUring {
        handle: IoringReactorHandle,
        pool: BufferPool,
        ring: SecondaryRingId,
    },
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
    let mut data = vec![0u8; length as usize];
    r.read_exact(&mut data)?;

    Ok(bincode::deserialize_from(&data[..])?)
}

impl PcidServerHandle {
    #[deprecated = "use connect_using_iouring instead"]
    pub fn connect_using_pipes(pcid_to_client: RawFd, pcid_from_client: RawFd) -> Result<Self> {
        Ok(Self {
            inner: PcidServerTransport::Pipe {
                pcid_to_client: unsafe { File::from_raw_fd(pcid_to_client) },
                pcid_from_client: unsafe { File::from_raw_fd(pcid_from_client) },
            },
        })
    }
    pub async fn connect_using_iouring(
        handle: IoringReactorHandle,
    ) -> Result<Self, IoUringSetupError> {
        log::debug!("creating instance");
        let instance = ConsumerInstanceBuilder::new()
            .with_submission_entry_count(64) // 4KiB, one page (minimum size)
            .with_completion_entry_count(128) // 4KiB, one page (minimum size)
            .create_instance()
            .map_err(IoUringSetupError::CreateInstanceError)?
            .map_all()
            .map_err(IoUringSetupError::MapAllError)?
            .attach("pci:")
            .map_err(IoUringSetupError::AttachError)?;
        log::debug!("created instance");

        log::debug!("adding sec instance");
        let ring = handle
            .reactor()
            .add_secondary_instance(instance, Priority::default())
            .map_err(IoUringSetupError::AddInstanceError)?;
        log::debug!("added sec instance");

        log::debug!("creating pool");
        let pool = handle
            .create_buffer_pool(ring, Priority::default(), 16384, ())
            .await
            .map_err(IoUringSetupError::BufferError)?;
        log::debug!("created pool");

        Ok(Self {
            inner: PcidServerTransport::IoUring { pool, ring, handle },
        })
    }

    #[deprecated = "use connect_using_iouring instead"]
    pub fn connect_using_pipes_from_env_fds() -> Result<Self> {
        let pcid_to_client_fd = env::var("PCID_TO_CLIENT_FD")?
            .parse::<RawFd>()
            .map_err(PcidClientHandleError::EnvValidityError)?;
        let pcid_from_client_fd = env::var("PCID_FROM_CLIENT_FD")?
            .parse::<RawFd>()
            .map_err(PcidClientHandleError::EnvValidityError)?;

        #[allow(deprecated)]
        Self::connect_using_pipes(pcid_to_client_fd, pcid_from_client_fd)
    }

    fn uses_pipes(&self) -> bool {
        matches!(self.inner, PcidServerTransport::Pipe { .. })
    }

    pub(crate) fn send(&mut self, req: &PcidClientRequest) -> Result<()> {
        match self.inner {
            PcidServerTransport::Pipe {
                ref pcid_from_client,
                ..
            } => send(&mut &*pcid_from_client, req),
            PcidServerTransport::IoUring { .. } => unreachable!(),
        }
    }
    pub(crate) fn recv(&mut self) -> Result<PcidClientResponse> {
        match self.inner {
            PcidServerTransport::Pipe {
                ref pcid_to_client, ..
            } => recv(&mut &*pcid_to_client),
            PcidServerTransport::IoUring { .. } => unreachable!(),
        }
    }
    pub async fn fetch_config(&mut self, priority: Priority) -> Result<SubdriverArguments> {
        if let PcidServerTransport::IoUring {
            ref handle,
            ref pool,
            ring,
        } = self.inner
        {
            let len = u32::try_from(mem::size_of::<SubdriverArguments>())
                .expect("SubdriverArguments has got too bloated");
            let alignment = u32::try_from(mem::align_of::<SubdriverArguments>())
                .expect("unexpected huge alignment for SubdriverArguments");

            log::debug!("LEN {} ALIGN {}", len, alignment);

            let mut slice = pool.acquire_borrowed_slice(len, alignment).ok_or(
                PcidClientHandleError::IoUringTransportError(Errno::new(ENOMEM)),
            )?;
            unsafe {
                let fut = handle.send(
                    ring,
                    SqEntry64 {
                        opcode: PcidOpcode::FetchConfig as u8,
                        priority,
                        syscall_flags: 1, // version
                        addr: slice.offset().into(),
                        len: len.into(),
                        fd: 0,
                        ..SqEntry64::default()
                    },
                );
                // Prevent data race by leaking memory if this future is forgotten using `mem::forget`.
                fut.guard(&mut slice);
                log::debug!("Future sent");

                let cqe = fut
                    .await
                    .map_err(PcidClientHandleError::IoUringTransportError)?;

                let result = Errno::demux64(cqe.status)
                    .map_err(PcidClientHandleError::IoUringTransportError)?;
                if result != 0 {
                    log::warn!("Expected zero as CQE return value when fetching config");
                }
                Ok(*plain::from_bytes(&*slice)
                    .expect("buffer pool allocator gave us an insufficient alignment"))
            }
        } else {
            self.send(&PcidClientRequest::RequestConfig)?;
            match self.recv()? {
                PcidClientResponse::Config(a) => Ok(a),
                other => Err(PcidClientHandleError::InvalidResponse(other)),
            }
        }
    }
    pub async fn fetch_all_capabilities(&mut self, priority: Priority) -> Result<Vec<Capability>> {
        if let PcidServerTransport::IoUring {
            ref handle,
            ref pool,
            ring,
        } = self.inner
        {
            let mut all_caps = Vec::new();

            let size: u32 = mem::size_of::<CapabilityRawTagged>().try_into().unwrap();
            let align: u32 = mem::align_of::<CapabilityRawTagged>().try_into().unwrap();

            let mut buffer = pool.acquire_borrowed_slice(size * 4, align).ok_or(
                PcidClientHandleError::IoUringTransportError(Errno::new(ENOMEM)),
            )?;

            loop {
                let caps_read = unsafe {
                    let fut = handle.send(
                        ring,
                        SqEntry64 {
                            opcode: PcidOpcode::FetchAllCapabilities as u8,
                            flags: 0,
                            priority,
                            user_data: 0, // overridden

                            syscall_flags: 1,
                            addr: 0, // unused
                            fd: 0,   // unused
                            len: (buffer.len() / size).into(),
                            offset: buffer.offset().into(),

                            additional1: 0, // unused
                            additional2: 0, // unused
                        },
                    );
                    fut.guard(&mut buffer);
                    let cqe = fut
                        .await
                        .map_err(PcidClientHandleError::IoUringTransportError)?;

                    Errno::demux64(cqe.status)
                        .map_err(PcidClientHandleError::IoUringTransportError)?
                };

                if caps_read == 0 {
                    break;
                }

                let bytes_read = usize::try_from(caps_read * u64::from(size)).or(Err(
                    PcidClientHandleError::IoUringTransportError(Errno::new(EOVERFLOW)),
                ))?;
                let caps = plain::slice_from_bytes::<CapabilityRawTagged>(&buffer[..bytes_read])
                    .expect("somehow redox_iou didn't consider alignment");

                all_caps.extend(
                    caps.into_iter()
                        .filter_map(|raw| Capability::construct(raw.kind, raw.raw)),
                );
            }
            Ok(all_caps)
        } else {
            self.send(&PcidClientRequest::GetCapabilities)?;
            match self.recv()? {
                PcidClientResponse::AllCapabilities(a) => Ok(a),
                other => Err(PcidClientHandleError::InvalidResponse(other)),
            }
        }
    }
    pub async fn get_capability(
        &mut self,
        ty: CapabilityType,
        priority: Priority,
    ) -> Result<Option<Capability>> {
        if let PcidServerTransport::IoUring {
            ref handle,
            ref pool,
            ring,
        } = self.inner
        {
            let size = mem::size_of::<CapabilityRawTagged>().try_into().unwrap();
            let align = mem::align_of::<CapabilityRawTagged>().try_into().unwrap();

            let mut buffer = pool.acquire_borrowed_slice(size, align).ok_or(
                PcidClientHandleError::IoUringTransportError(Errno::new(ENOMEM)),
            )?;

            unsafe {
                let fut = handle.send(
                    ring,
                    SqEntry64 {
                        opcode: PcidOpcode::GetCapability as u8,
                        flags: 0,
                        priority,
                        user_data: 0, // overridden

                        syscall_flags: 1,
                        fd: 0, // unused
                        len: size.into(),
                        addr: 0, // FIXME
                        offset: buffer.offset().into(),

                        additional1: 0, // unused
                        additional2: 0, // unused
                    },
                );
                fut.guard(&mut buffer);
                let cqe = fut
                    .await
                    .map_err(PcidClientHandleError::IoUringTransportError)?;
                Errno::demux64(cqe.status).map_err(PcidClientHandleError::IoUringTransportError)?;
            }

            let raw_tagged = *plain::from_bytes::<CapabilityRawTagged>(&*buffer)
                .expect("somehow redox_iou gave us an insufficient alignment");

            Ok(Capability::construct(raw_tagged.kind, raw_tagged.raw))
        } else {
            self.send(&PcidClientRequest::GetCapability(ty))?;
            match self.recv()? {
                PcidClientResponse::Capability(c) => Ok(c),
                other => Err(PcidClientHandleError::InvalidResponse(other)),
            }
        }
    }
    pub async fn set_capability(
        &mut self,
        info: SetCapabilityInfo,
        priority: Priority,
    ) -> Result<()> {
        if let PcidServerTransport::IoUring {
            ref handle,
            ref pool,
            ring,
        } = self.inner
        {
            let size = mem::size_of::<SetCapabilityInfoRaw>()
                .try_into()
                .or(Err(Errno::new(EOVERFLOW)))
                .map_err(PcidClientHandleError::IoUringTransportError)?;
            let align = mem::align_of::<SetCapabilityInfoRaw>()
                .try_into()
                .or(Err(Errno::new(EOVERFLOW)))
                .map_err(PcidClientHandleError::IoUringTransportError)?;

            let mut slice = pool.acquire_borrowed_slice(size, align).ok_or(
                PcidClientHandleError::IoUringTransportError(Errno::new(ENOMEM)),
            )?;

            let set_info = plain::from_mut_bytes(&mut *slice)
                .expect("expected redox_iou to give us the correct alignment");

            *set_info = match info {
                SetCapabilityInfo::Msi(info) => SetCapabilityInfoRaw {
                    id: CapabilityType::Msi as u32,
                    inner: SetCapabilityInfoInner { msi: info },
                },
                SetCapabilityInfo::MsiX(info) => SetCapabilityInfoRaw {
                    id: CapabilityType::MsiX as u32,
                    inner: SetCapabilityInfoInner { msix: info },
                },
            };

            unsafe {
                let fut = handle.send(
                    ring,
                    SqEntry64 {
                        opcode: PcidOpcode::SetCapability as u8,
                        flags: 0,
                        priority,
                        user_data: 0,

                        syscall_flags: 1,
                        addr: 0, // TODO: Remove this
                        len: size.into(),
                        fd: 0, // unused
                        offset: slice.offset().into(),

                        additional1: 0,
                        additional2: 0,
                    },
                );
                fut.guard(&mut slice);
                let cqe = fut
                    .await
                    .map_err(PcidClientHandleError::IoUringTransportError)?;
                let _ = Errno::demux64(cqe.status)
                    .map_err(PcidClientHandleError::IoUringTransportError)?;
            }
            Ok(())
        } else {
            self.send(&PcidClientRequest::SetCapability(info))?;
            match self.recv()? {
                PcidClientResponse::SetCapability => Ok(()),
                other => Err(PcidClientHandleError::InvalidResponse(other)),
            }
        }
    }
}

/// A 32-bit PCI function address.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PciAddress32 {
    /// The PCI Segment Group, as described by the "PCI Firmware Specification rev 3.0". TL;DR it's
    /// a PCIe-specific extension to the regular 8-bits bus numbers.
    pub seg_group: u16,
    /// The bus number, which according to the "PCI Express Base Specification rev 3.0", is 1-8
    /// bits wide.
    pub bus: u8,
    /// The device and function number, which according to the aforementioned PCIe spec (ECAM
    /// section), is five bits wide at the moment. The device number is bits 7:5 of this field,
    /// whereas the function number is bits 4:0.
    pub dev_and_fun: u8,
}
impl PciAddress32 {
    pub const fn seg_group(&self) -> u16 {
        self.seg_group
    }
    pub const fn bus(&self) -> u8 {
        self.bus
    }
    pub const fn device(&self) -> u8 {
        (self.dev_and_fun & 0xE0) >> 5
    }
    pub const fn function(&self) -> u8 {
        self.dev_and_fun & 0x07
    }

    pub fn set_seg_group(&mut self, group: u16) {
        self.seg_group = group;
    }
    pub fn set_bus(&mut self, bus: u8) {
        self.bus = bus;
    }
    pub fn set_device(&mut self, dev: u8) {
        assert_eq!((dev << 5) & 0xE0, (dev << 5), "device field is too large");
        self.dev_and_fun &= 0x7F;
        self.dev_and_fun |= dev << 5;
    }
    pub fn set_function(&mut self, func: u8) {
        assert_eq!(func & 0x7F, func, "function field is too large");
        self.dev_and_fun &= 0xE0;
        self.dev_and_fun |= func;
    }
    pub const fn with_seg_group(self, seg_group: u16) -> Self {
        Self { seg_group, ..self }
    }
    pub const fn with_bus(self, bus: u8) -> Self {
        Self { bus, ..self }
    }
    pub fn with_device(mut self, dev: u8) -> Self {
        self.set_device(dev);
        self
    }
    pub fn with_function(mut self, func: u8) -> Self {
        self.set_function(func);
        self
    }
}
unsafe impl plain::Plain for PciAddress32 {}

/// A 64-bit PCI function address, that apart from the regular [`PciAddress32`], only contains an
/// extra field if PCI addresses were to get bigger in the future (or if they already have in PCI
/// 4.0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PciAddress64 {
    /// The lower 32 bits of the `fd` field of the SQE, that provides exactly the number of bits
    /// required by PCIe 3.0 to store a full PCI function address.
    pub base: PciAddress32,
    /// Used to store 32 additional bits of information, to support future versions of PCI or
    /// similar that could use larger addresses, or for vendor-specific information.
    pub extra: u32,
}
unsafe impl plain::Plain for PciAddress64 {}

pub const PCID_OPCODE_FLAG_DEF_DEV: u32 = 1 << 31;

/// The producer-specific opcodes that `pcid` uses for low-overhead IPC.
///
/// The `syscall_flags` field is currently always the version (`1` at the moment). The `fd` field
/// is mainly for specifying addresses to PCI functions, since pcid cannot immediately know when it
/// receives an io_uring, which device the IPC will actually be targeting.
// TODO: Get a proper permission system to limit the information that a subdriver can receive about
// other devices.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PcidOpcode {
    /// Fetch the PCI config, containing all necessary subdriver arguments. This information will
    /// be written to a [`SubdriverArguments` ]struct within a shared buffer pool. Uses all implicit
    /// fields.
    ///
    /// # Parameters
    ///
    /// | required field | usage description                                         | SIZE |
    /// |----------------|-----------------------------------------------------------|------|
    /// | syscall_flags  | the version of this API to use (currently 1)              | BOTH |
    /// | addr           | not used                                                  | BOTH |
    /// | len            | the length of that struct                                 | BOTH |
    /// | fd             | the address of the PCI function                           | BOTH |
    /// | offset         | the offset within the buffer pool, to write into          | BOTH |
    /// | additional1    | not used                                                  | 64   |
    /// | additional2    | not used                                                  | 64   |
    ///
    /// # Return value
    ///
    /// The return value of this opcode, is a Result of usize, which is always zero or error.
    ///
    /// # Errors (non-exhausive, all error conditions may not be checked for)
    ///
    /// * `ENOSYS` - the version field is unsupported (at the moment: not equal to 1)
    /// * `EBADF` - the index of shared buffer pool that was inputted, was invalid
    /// * `EINVAL` - the length field was insufficient to store a [`SubdriverArguments`].
    /// * `EFAULT` - the offset was outside the pool limit
    /// * `EADDRINUSE` - the offset+len pair overlapped an already-in-use address of the pool
    ///
    FetchConfig = 128,

    /// Fetch one or more PCI capabilities, with some extra data attached to them. Uses all
    /// implicit fields.
    ///
    /// # Parameters
    ///
    /// | required field | usage description                                         | SIZE |
    /// |----------------|-----------------------------------------------------------|------|
    /// | syscall_flags  | the version of this API to use (currently 1)              | BOTH |
    /// | addr           | the start index of the capabilities to read               | BOTH |
    /// | len            | the number of capabilities to read                        | BOTH |
    /// | fd             | the address of the PCI function                           | BOTH |
    /// | offset         | the offset within that buffer pool, to write into         | BOTH |
    /// | additional1    | not used                                                  | 64   |
    /// | additional2    | not used                                                  | 64   |
    ///
    /// # Return value
    ///
    /// The return value of this opcode, is a Result of usize, indicating the number of
    /// capabilities read. If the extra field is present, it will contain the number of
    /// capabilities left to read.
    ///
    /// # Errors (non-exhausive, all error conditions may not be checked for)
    ///
    /// * `ENOSYS` - the version field is unsupported (at the moment: not equal to 1)
    /// * `EFAULT` - the offset was outside the pool limit
    /// * `EADDRINUSE` - the offset+len pair overlapped an already-in-use address of the pool
    ///
    FetchAllCapabilities,

    /// Get the current static and runtime parameters of a specific capability. Uses all implicit
    /// fields.
    ///
    /// # Parameters
    ///
    /// | required field | usage description                                         | SIZE |
    /// |----------------|-----------------------------------------------------------|------|
    /// | syscall_flags  | the version of this API to use (currently 1)              | BOTH |
    /// | addr           | the index of the capability to read                       | BOTH |
    /// | len            | the size of the buffer to write the capability into       | BOTH |
    /// | fd             | the offset of the PCI function                            | BOTH |
    /// | offset         | the offset within that buffer pool, to write into         | BOTH |
    /// | additional1    | not used                                                  | 64   |
    /// | additional2    | not used                                                  | 64   |
    ///
    /// # Return value
    ///
    /// The return value of this opcode, is a Result of usize, which is the byte size of the
    /// capability struct.
    ///
    /// # Errors (non-exhausive, all error conditions may not be checked for)
    ///
    /// * `ENOSYS` - the version field is unsupported (at the moment: not equal to 1)
    /// * `EFAULT` - the offset was outside the pool limit
    /// * `EADDRINUSE` - the offset+len pair overlapped an already-in-use address of the pool
    /// * `ENOENT` - the index of the capability to read was non-existent
    ///
    GetCapability,

    /// Set capability parameters for a specific capability. Uses all implicit fields.
    ///
    /// # Parameters
    ///
    /// | required field | usage description                                         | SIZE |
    /// |----------------|-----------------------------------------------------------|------|
    /// | syscall_flags  | the version of this API to use (currently 1)              | BOTH |
    /// | addr           | the index of the capability to modify                     | BOTH |
    /// | len            | the size of the buffer to modify the capability from      | BOTH |
    /// | fd             | the address of the PCI function                           | BOTH |
    /// | offset         | the offset within that buffer pool, to write into         | BOTH |
    /// | additional1    | not used                                                  | 64   |
    /// | additional2    | not used                                                  | 64   |
    ///
    /// # Return value
    ///
    /// The return value of this opcode, is a Result of usize, is always zero or error.
    ///
    /// # Errors (non-exhausive, all error conditions may not be checked for)
    ///
    /// * `ENOSYS` - the version field is unsupported (at the moment: not equal to 1)
    /// * `EFAULT` - the offset was outside the pool limit
    /// * `EADDRINUSE` - the offset+len pair overlapped an already-in-use address of the pool
    /// * `ENOENT` - the index of the capability to write was non-existent
    /// * `EBADMSG` - the set capability data has malformed, or used an unsupported capability ID.
    ///
    SetCapability,
}

impl PcidOpcode {
    pub fn from_raw(raw: u8) -> Option<Self> {
        Some(if raw == Self::FetchConfig as u8 {
            Self::FetchConfig
        } else if raw == Self::FetchAllCapabilities as u8 {
            Self::FetchAllCapabilities
        } else if raw == Self::GetCapability as u8 {
            Self::GetCapability
        } else if raw == Self::SetCapability as u8 {
            Self::SetCapability
        } else {
            return None;
        })
    }
}
