use std::collections::{BTreeMap, VecDeque};

use pci_types::{ConfigRegionAccess, PciAddress};
use redox_scheme::scheme::SchemeSync;
use redox_scheme::{CallerCtx, OpenResult};
use syscall::dirent::{DirEntry, DirentBuf, DirentKind};
use syscall::error::{Error, Result, EACCES, EBADF, EINVAL, EIO, EISDIR, ENOENT, ENOTDIR};
use syscall::flag::{MODE_CHR, MODE_DIR, O_DIRECTORY, O_STAT};
use syscall::schemev2::NewFdFlags;
use syscall::ENOLCK;

use crate::cfg_access::Pcie;

pub struct PciScheme {
    handles: BTreeMap<usize, HandleWrapper>,
    next_id: usize,
    pcie: Pcie,
    tree: BTreeMap<PciAddress, crate::Func>,
}
enum Handle {
    TopLevel { entries: Vec<String> },
    Access,
    Device,
    Channel { addr: PciAddress, st: ChannelState },
}
struct HandleWrapper {
    inner: Handle,
    stat: bool,
}
impl Handle {
    fn is_file(&self) -> bool {
        matches!(self, Self::Access | Self::Channel { .. })
    }
    fn is_dir(&self) -> bool {
        !self.is_file()
    }
    // TODO: capability rather than root
    fn requires_root(&self) -> bool {
        matches!(self, Self::Access | Self::Channel { .. })
    }
}

enum ChannelState {
    AwaitingData,
    AwaitingResponseRead(VecDeque<u8>),
}

const DEVICE_CONTENTS: &[&str] = &["channel"];

impl SchemeSync for PciScheme {
    fn open(&mut self, path: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        log::trace!("OPEN `{}` flags {}", path, flags);

        // TODO: Check flags are correct
        let expects_dir = path.ends_with('/') || flags & O_DIRECTORY != 0;

        let path = path.trim_matches('/');

        let handle = if path.is_empty() {
            Handle::TopLevel {
                entries: self
                    .tree
                    .iter()
                    // FIXME remove replacement of : once the old scheme format is no longer supported.
                    .map(|(addr, _)| format!("{}", addr).replace(':', "--"))
                    .collect::<Vec<_>>(),
            }
        } else if path == "access" {
            Handle::Access
        } else {
            let idx = path.find('/').unwrap_or(path.len());
            let (addr_str, after) = path.split_at(idx);
            let addr = parse_pci_addr(addr_str).ok_or(Error::new(ENOENT))?;

            self.parse_after_pci_addr(addr, after)?
        };

        let stat = flags & O_STAT != 0;
        if expects_dir && handle.is_file() && !stat {
            return Err(Error::new(ENOTDIR));
        }
        if !expects_dir && handle.is_dir() && !stat {
            return Err(Error::new(EISDIR));
        }
        if ctx.uid != 0 && handle.requires_root() && !stat {
            return Err(Error::new(EACCES));
        }

        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(
            id,
            HandleWrapper {
                inner: handle,
                stat,
            },
        );
        Ok(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        })
    }
    fn fstat(&mut self, id: usize, stat: &mut syscall::Stat, _ctx: &CallerCtx) -> Result<()> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let (len, mode) = match handle.inner {
            Handle::TopLevel { ref entries } => (entries.len(), MODE_DIR | 0o755),
            Handle::Device => (DEVICE_CONTENTS.len(), MODE_DIR | 0o755),
            Handle::Access | Handle::Channel { .. } => (0, MODE_CHR | 0o600),
        };
        stat.st_size = len as u64;
        stat.st_mode = mode;
        Ok(())
    }
    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        match handle.inner {
            Handle::TopLevel { .. } => Err(Error::new(EISDIR)),
            Handle::Device => Err(Error::new(EISDIR)),
            Handle::Channel {
                addr: _,
                ref mut st,
            } => Self::read_channel(st, buf),
            _ => Err(Error::new(EBADF))
        }
    }
    fn getdents<'buf>(
        &mut self,
        id: usize,
        mut buf: DirentBuf<&'buf mut [u8]>,
        opaque_offset: u64,
    ) -> Result<DirentBuf<&'buf mut [u8]>> {
        let Ok(offset) = usize::try_from(opaque_offset) else {
            return Ok(buf);
        };

        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        let entries = match handle.inner {
            Handle::TopLevel { ref entries } => {
                for (i, dent_name) in entries.iter().enumerate().skip(offset) {
                    buf.entry(DirEntry {
                        inode: 0,
                        name: dent_name,
                        kind: DirentKind::Unspecified,
                        next_opaque_id: i as u64 + 1,
                    })?;
                }
                return Ok(buf);
            }
            Handle::Device => DEVICE_CONTENTS,
            Handle::Access | Handle::Channel { .. } => return Err(Error::new(ENOTDIR)),
        };

        for (i, dent_name) in entries.iter().enumerate().skip(offset) {
            buf.entry(DirEntry {
                inode: 0,
                name: dent_name,
                kind: DirentKind::Unspecified,
                next_opaque_id: i as u64 + 1,
            })?;
        }
        Ok(buf)
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        match handle.inner {
            Handle::Channel { addr, ref mut st } => {
                Self::write_channel(&self.pcie, &mut self.tree, addr, st, buf)
            }

            _ => Err(Error::new(EBADF)),
        }
    }

    fn call(&mut self, id: usize, payload: &mut [u8], metadata: &[u64]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        match handle.inner {
            Handle::Access => {
                let payload_len = u16::try_from(payload.len()).map_err(|_| Error::new(EINVAL))?;
                let write = match metadata.get(0) {
                    Some(1) => false,
                    Some(2) => true,
                    _ => return Err(Error::new(EINVAL)),
                };
                let (addr, offset) = match metadata.get(1) {
                    Some(value) => {
                        // Segment: u16, at 28 bits
                        // Bus: u8, 8 bits, 256 total, at 20 bits
                        // Device: u8, 5 bits, 32 total, at 15 bits
                        // Function: u8, 3 bits, 8 total, at 12 bits
                        // Offset: u16, 12 bits, 4096 total, at 0 bits
                        (
                            PciAddress::new(
                                ((value >> 28) & 0xFFFF) as u16,
                                ((value >> 20) & 0xFF) as u8,
                                ((value >> 15) & 0x1F) as u8,
                                ((value >> 12) & 0x7) as u8,
                            ),
                            (value & 0xFFF) as u16
                        )
                    }
                    None => return Err(Error::new(EINVAL)),
                };
                // This handle must allow less than 4 byte access, but the
                // lower level only works with 4 byte reads and writes
                let unaligned = offset % 4;
                let start = offset - unaligned;
                let end = offset + payload_len;
                let mut i = 0;
                while start + i < end {
                    let mut bytes = unsafe { self.pcie.read(addr, start + i) }.to_le_bytes();
                    for j in 0..bytes.len() {
                        if let Some(payload_i) = i.checked_sub(unaligned) {
                            if let Some(payload_b) = payload.get_mut(usize::from(payload_i)) {
                                if write {
                                    bytes[j] = *payload_b;
                                } else {
                                    *payload_b = bytes[j]
                                }
                            }
                        }
                        i += 1;
                    }
                    if write {
                        let value = u32::from_le_bytes(bytes);
                        unsafe { self.pcie.write(addr, start + i, value); }
                    }
                }
                Ok(payload.len())
            }

            _ => Err(Error::new(EBADF)),
        }
    }
}

impl PciScheme {
    pub fn on_close(&mut self, id: usize) {
        match self.handles.remove(&id) {
            Some(HandleWrapper {
                inner: Handle::Channel { addr, .. },
                ..
            }) => {
                log::trace!("TODO: Support disabling device (called on {})", addr);
                if let Some(func) = self.tree.get_mut(&addr) {
                    func.enabled = false;
                }
            }
            _ => {}
        }
    }
}

impl PciScheme {
    pub fn new(pcie: Pcie, tree: BTreeMap<PciAddress, crate::Func>) -> Self {
        Self {
            handles: BTreeMap::new(),
            next_id: 0,
            pcie,
            tree,
        }
    }
    fn parse_after_pci_addr(&mut self, addr: PciAddress, after: &str) -> Result<Handle> {
        if after.chars().next().map_or(false, |c| c != '/') {
            return Err(Error::new(ENOENT));
        }
        let func = self.tree.get_mut(&addr).ok_or(Error::new(ENOENT))?;

        Ok(if after.is_empty() {
            Handle::Device
        } else {
            let path = &after[1..];

            match path {
                "channel" => {
                    if func.enabled {
                        return Err(Error::new(ENOLCK));
                    }
                    func.inner.legacy_interrupt_line = crate::enable_function(
                        &self.pcie,
                        &mut func.endpoint_header,
                        &mut func.capabilities,
                    );
                    func.enabled = true;
                    Handle::Channel {
                        addr,
                        st: ChannelState::AwaitingData,
                    }
                }
                _ => return Err(Error::new(ENOENT)),
            }
        })
    }

    fn read_channel(state: &mut ChannelState, buf: &mut [u8]) -> Result<usize> {
        match *state {
            ChannelState::AwaitingResponseRead(ref mut queue) => {
                let byte_count = std::cmp::min(queue.len(), buf.len());
                // XXX: Why can't VecDeque support dequeueing into slices?
                for (idx, byte) in queue.drain(..byte_count).enumerate() {
                    buf[idx] = byte;
                }
                if queue.is_empty() {
                    *state = ChannelState::AwaitingData;
                }
                Ok(byte_count)
            }
            ChannelState::AwaitingData => Err(Error::new(EINVAL)),
        }
    }
    fn write_channel(
        pci_state: &Pcie,
        tree: &mut BTreeMap<PciAddress, crate::Func>,
        addr: PciAddress,
        state: &mut ChannelState,
        buf: &[u8],
    ) -> Result<usize> {
        match *state {
            ChannelState::AwaitingResponseRead(_) => return Err(Error::new(EINVAL)),
            ChannelState::AwaitingData => {
                let func = tree.get_mut(&addr).unwrap();

                let request = bincode::deserialize_from(buf).map_err(|_| Error::new(EINVAL))?;
                let response = crate::driver_handler::DriverHandler::new(
                    func.inner.clone(),
                    &mut func.endpoint_header,
                    &mut func.capabilities,
                    &*pci_state,
                )
                .respond(request);

                let mut output_bytes = vec![0_u8; 8];
                bincode::serialize_into(&mut output_bytes, &response)
                    .map_err(|_| Error::new(EIO))?;
                let len = output_bytes.len() - 8;
                output_bytes[..8].copy_from_slice(&u64::to_le_bytes(len as u64));
                *state = ChannelState::AwaitingResponseRead(output_bytes.into());

                Ok(buf.len())
            }
        }
    }
}

fn parse_pci_addr(addr: &str) -> Option<PciAddress> {
    let (segment, rest) = addr.split_once('-')?;
    let segment = u16::from_str_radix(segment, 16).ok()?;

    // FIXME use : instead of -- as separator once the old scheme format is no longer supported.
    let (bus, rest) = rest.split_once("--")?;
    let bus = u8::from_str_radix(bus, 16).ok()?;

    let (device, function) = rest.split_once('.')?;
    let device = u8::from_str_radix(device, 16).ok()?;
    let function = u8::from_str_radix(function, 16).ok()?;

    Some(PciAddress::new(segment, bus, device, function))
}
