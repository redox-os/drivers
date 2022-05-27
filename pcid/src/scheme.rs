use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;

use syscall::error::{Error, Result, EACCES, EBADF, EBADFD, EINVAL, EIO, EISDIR, ENOENT, ENOTDIR, ESPIPE};
use syscall::flag::{MODE_CHR, MODE_DIR, MODE_FILE, O_DIRECTORY, O_STAT};
use syscall::scheme::SchemeMut;

use crate::{PciAddr, State};

pub struct PciScheme {
    handles: BTreeMap<usize, HandleWrapper>,
    next_id: usize,
    state: State,
    tree: BTreeMap<PciAddr, crate::Func>,
}
enum Handle {
    TopLevel { offset: usize },
    Tree { offset: usize, bytes: Vec<u8> },
    Device { offset: usize },
    CfgSpace { offset: usize, addr: PciAddr },
    Channel { addr: PciAddr, st: ChannelState },
    DeviceProperty { offset: usize, property: String },
    Enabled { offset: usize, addr: PciAddr },
}
struct HandleWrapper {
    inner: Handle,
    stat: bool,
}
impl Handle {
    fn is_file(&self) -> bool {
        matches!(self, Self::CfgSpace { .. } | Self::Channel { .. } | Self::DeviceProperty { .. } | Self::Enabled { .. })
    }
    fn is_dir(&self) -> bool {
        !self.is_file()
    }
    // TODO: capability rather than root
    fn requires_root(&self) -> bool {
        matches!(self, Self::CfgSpace { .. } | Self::Channel { .. })
    }
}

enum ChannelState {
    AwaitingLenBytes(arrayvec::ArrayVec<u8, 8>),
    AwaitingData(usize, Vec<u8>),
    AwaitingResponseRead(VecDeque<u8>),
}

const ROOT_CONTENTS: &[u8] = b"tree\n";
const DEVICE_CONTENTS: &[u8] = br"cfg-space
channel
vendor-id
device-id
class
subclass
interface
interrupt-line
revision
bars
bar-sizes
enabled
";

impl SchemeMut for PciScheme {
    fn open(&mut self, path: &str, flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        log::trace!("OPEN `{}` flags {}", path, flags);

        // TODO: Check flags are correct
        let expects_dir = path.ends_with('/') || flags & O_DIRECTORY != 0;

        let path = path.trim_matches('/');

        let handle = if path.is_empty() {
            Handle::TopLevel { offset: 0 }
        } else if path.starts_with("tree") {
            let path = &path[4..];

            if path.chars().next().map_or(false, |c| c != '/') {
                return Err(Error::new(ENOENT));
            }

            if path.is_empty() {
                Handle::Tree { offset: 0, bytes: self.tree.iter().flat_map(|(addr, _)| format!("{}\n", addr).into_bytes()).collect::<Vec<_>>() }
            } else {
                let path = &path[1..];

                let idx = path.find('/').unwrap_or(path.len());
                let (addr_str, after) = path.split_at(idx);
                let addr = addr_str.parse::<PciAddr>().map_err(|_| Error::new(ENOENT))?;

                self.parse_after_pci_addr(addr, after).ok_or(Error::new(ENOENT))?
            }
        } else {
            return Err(Error::new(ENOENT))?;
        };

        let stat = flags & O_STAT != 0;
        if expects_dir && handle.is_file() && !stat {
            return Err(Error::new(ENOTDIR));
        }
        if !expects_dir && handle.is_dir() && !stat {
            return Err(Error::new(EISDIR));
        }
        if uid != 0 && handle.requires_root() && !stat {
            return Err(Error::new(EACCES));
        }

        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(id, HandleWrapper { inner: handle, stat });
        Ok(id)
    }
    fn fstat(&mut self, id: usize, stat: &mut syscall::Stat) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let (len, mode) = match handle.inner {
            Handle::TopLevel { .. } => (ROOT_CONTENTS.len(), MODE_DIR | 0o755),
            Handle::Tree { ref bytes, .. } => (bytes.len(), MODE_DIR | 0o755),
            Handle::Device { .. } => (DEVICE_CONTENTS.len(), MODE_DIR | 0o755),
            Handle::CfgSpace { .. } => (Self::cfg_space_len(&self.state), MODE_CHR | 0o600),
            Handle::Channel { .. } => (0, MODE_CHR | 0o600),
            Handle::DeviceProperty { ref property, .. } => (property.len(), MODE_FILE | 0o400),
            Handle::Enabled { .. } => (1, MODE_FILE | 0o600),
        };
        stat.st_size = len as u64;
        stat.st_mode = mode;
        Ok(0)
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat { return Err(Error::new(EBADF)); }

        let (offset, bytes) = match handle.inner {
            Handle::CfgSpace { ref mut offset, addr } => return Self::read_cfgspace(&self.state, offset, addr, buf),
            Handle::TopLevel { ref mut offset } => (offset, ROOT_CONTENTS),
            Handle::Tree { ref mut offset, ref bytes } => (offset, bytes.as_slice()),
            Handle::Device { ref mut offset } => (offset, DEVICE_CONTENTS),
            Handle::Channel { addr, ref mut st } => return Self::read_channel(addr, st, buf),
            Handle::DeviceProperty { ref mut offset, ref property } =>  (offset, property.as_bytes()),
            Handle::Enabled { ref mut offset, addr } => {
                let is_enabled = self.tree.get(&addr).ok_or(Error::new(EBADF))?.enabled;
                (offset, (if is_enabled { b"1" } else { b"0" }).as_slice())
            }
        };

        let byte_count = std::cmp::min(bytes.len().saturating_sub(*offset), buf.len());
        buf[..byte_count].copy_from_slice(&bytes[*offset..*offset + byte_count]);
        *offset += byte_count;

        Ok(byte_count)
    }
    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat { return Err(Error::new(EBADF)); }

        match handle.inner {
            Handle::CfgSpace { ref mut offset, addr } => Self::write_cfgspace(&self.state, offset, addr, buf),
            Handle::Channel { addr, ref mut st } => Self::write_channel(&self.state, &mut self.tree, addr, st, buf),
            Handle::Enabled { ref mut offset, addr } => Self::set_enabled(&self.state, self.tree.get_mut(&addr).ok_or(Error::new(EBADFD))?, addr, offset, buf),

            _ => Err(Error::new(EBADF)),
        }
    }
    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat { return Err(Error::new(EBADF)); }

        let (offset, len) = match &mut handle.inner {
            Handle::Tree { offset, bytes, .. } => (offset, bytes.len()),
            Handle::TopLevel { offset } => (offset, ROOT_CONTENTS.len()),
            Handle::Device { offset } => (offset, DEVICE_CONTENTS.len()),
            Handle::CfgSpace { offset, .. } => (offset, Self::cfg_space_len(&self.state)),
            Handle::Channel { .. } => return Err(Error::new(ESPIPE)),
            Handle::DeviceProperty { offset, property } => (offset, property.len()),
            Handle::Enabled { offset, .. } => (offset, 1),
        };

        *offset = syscall::calc_seek_offset_usize(*offset, pos, whence, len)? as usize;
        Ok(*offset as isize)
    }
    fn close(&mut self, id: usize) -> Result<usize> {
        let _ = self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
}

impl PciScheme {
    pub fn new(state: State, tree: BTreeMap<PciAddr, crate::Func>) -> Self {
        Self {
            handles: BTreeMap::new(),
            next_id: 0,
            state,
            tree,
        }
    }
    fn set_enabled(state: &State, func: &mut crate::Func, addr: PciAddr, file_offset: &mut usize, buf: &[u8]) -> Result<usize> {
        if *file_offset > 0 || buf.is_empty() { return Ok(0); }

        let enable = match buf[0] {
            b'1' => true,
            b'0' => false,
            _ => return Err(Error::new(EINVAL)),
        };

        if enable {
            crate::enable_func(state.preferred_cfg_access(), addr, func);
        } else {
            log::warn!("TODO: Support disabling device (called on {})", addr);
        }

        Ok(1)
    }
    fn parse_after_pci_addr(&self, addr: PciAddr, after: &str) -> Option<Handle> {
        if after.chars().next().map_or(false, |c| c != '/') {
            return None;
        }
        let func = self.tree.get(&addr)?;

        Some(if after.is_empty() {
            Handle::Device { offset: 0 }
        } else {
            let path = &after[1..];

            let p = |property| Handle::DeviceProperty { offset: 0, property };

            match path {
                "cfg-space" => Handle::CfgSpace { offset: 0, addr },
                "channel" => Handle::Channel { addr, st: ChannelState::AwaitingLenBytes(arrayvec::ArrayVec::new()) },
                // TODO: Hex or dec?
                "vendor-id" => p(format!("{:>04X}", func.header.vendor_id())),
                "device-id" => p(format!("{:>04X}", func.header.device_id())),
                "class" => p(u8::from(func.header.class()).to_string()),
                "subclass" => p(func.header.subclass().to_string()),
                "interface" => p(func.header.interface().to_string()),
                "interrupt-line" => p(func.header.interrupt_line().to_string()),
                "revision" => p(func.header.revision().to_string()),
                "bars" => p({
                    let mut s = String::new();
                    for (bar, _) in func.bars {
                        writeln!(s, "{}", bar).unwrap();
                    }
                    s
                }),
                "bar-sizes" => p({
                    let mut s = String::new();
                    for (_, bar_size) in func.bars {
                        writeln!(s, "{:>08X}", bar_size).unwrap();
                    }
                    s
                }),
                "enabled" => Handle::Enabled { offset: 0, addr },
                _ => return None,
            }
        })
    }
    fn cfg_space_len(state: &State) -> usize {
        if state.pcie.is_some() { 4096 } else { 256 }
    }
    // TODO: I have tested these manually, but write an automated test just in case.
    fn read_cfgspace(state: &State, file_offset: &mut usize, addr: PciAddr, buf: &mut [u8]) -> Result<usize> {
        let mut offset = *file_offset;
        let byte_count = std::cmp::min(Self::cfg_space_len(state).saturating_sub(offset), buf.len());
        let buf = &mut buf[..byte_count];

        let displacement = offset % 4;

        let buf = if displacement != 0 {
            let dw_bytes = u32::to_le_bytes(unsafe { state.preferred_cfg_access().read(addr, ((offset / 4) * 4) as u16) });
            let count = std::cmp::min(buf.len(), 4 - displacement);
            buf[..count].copy_from_slice(&dw_bytes[displacement..displacement + count]);
            offset += count;
            &mut buf[count..]
        } else { buf };

        for dst_dw_bytes in buf.array_chunks_mut::<4>() {
            *dst_dw_bytes = u32::to_le_bytes(unsafe { state.preferred_cfg_access().read(addr, offset as u16) });
            offset += 4;
        }

        let tail_byte_count = buf.len() % 4;
        if tail_byte_count != 0 {
            let dw_bytes = u32::to_le_bytes(unsafe { state.preferred_cfg_access().read(addr, offset as u16) });
            buf[byte_count - tail_byte_count..].copy_from_slice(&dw_bytes[..tail_byte_count]);
            offset += tail_byte_count;
        }

        *file_offset = offset;
        Ok(byte_count)
    }
    fn write_cfgspace(state: &State, file_offset: &mut usize, addr: PciAddr, buf: &[u8]) -> Result<usize> {
        let mut offset = *file_offset;
        let byte_count = std::cmp::min(Self::cfg_space_len(state).saturating_sub(offset), buf.len());
        let buf = &buf[..byte_count];

        let displacement = offset % 4;

        let buf = if displacement != 0 {
            let (bytes_before, bytes) = if buf.len() >= 4 - displacement { buf.split_at(4 - displacement) } else { (buf, [].as_slice()) };
            let mut dw_bytes = [0_u8; 4];
            let count = std::cmp::min(bytes_before.len(), 4 - displacement);
            dw_bytes[displacement..displacement + count].copy_from_slice(&bytes_before[..count]);
            unsafe { state.preferred_cfg_access().write(addr, ((offset / 4) * 4) as u16, u32::from_le_bytes(dw_bytes)); }

            offset += count;

            bytes
        } else { buf };

        for dword in buf.array_chunks::<4>().copied().map(u32::from_le_bytes) {
            unsafe { state.preferred_cfg_access().write(addr, offset as u16, dword); }
            offset += 4;
        }

        if buf.len() % 4 != 0 {
            let mut dw_bytes = [0_u8; 4];
            dw_bytes[..buf.len() % 4].copy_from_slice(&buf[(buf.len() / 4) * 4..]);
            unsafe { state.preferred_cfg_access().write(addr, offset as u16, u32::from_le_bytes(dw_bytes)) }

            offset += buf.len() % 4;
        }
        *file_offset = offset;
        Ok(byte_count)
    }

    fn read_channel(addr: PciAddr, state: &mut ChannelState, buf: &mut [u8]) -> Result<usize> {
        match *state {
            ChannelState::AwaitingResponseRead(ref mut queue) => {
                let byte_count = std::cmp::min(queue.len(), buf.len());
                // XXX: Why can't VecDeque support dequeueing into slices?
                for (idx, byte) in queue.drain(..byte_count).enumerate() {
                    buf[idx] = byte;
                }
                if queue.is_empty() {
                    *state = ChannelState::AwaitingLenBytes(arrayvec::ArrayVec::new());
                }
                Ok(byte_count)
            }
            ChannelState::AwaitingLenBytes(_) | ChannelState::AwaitingData(_, _) => Err(Error::new(EINVAL)),
        }
    }
    fn write_channel(pci_state: &State, tree: &mut BTreeMap<PciAddr, crate::Func>, addr: PciAddr, state: &mut ChannelState, buf: &[u8]) -> Result<usize> {
        match *state {
            ChannelState::AwaitingResponseRead(_) => return Err(Error::new(EINVAL)),
            ChannelState::AwaitingLenBytes(ref mut len_bytes) => {
                let byte_count = std::cmp::min(len_bytes.capacity() - len_bytes.len(), buf.len());
                len_bytes.try_extend_from_slice(&buf[..byte_count]).unwrap();

                if let Ok(len_bytes) = len_bytes.clone().into_inner() {
                    // TODO: Validate length
                    let len = u64::from_le_bytes(len_bytes) as usize;
                    *state = ChannelState::AwaitingData(len, Vec::with_capacity(len));
                }

                Ok(byte_count)
            }
            ChannelState::AwaitingData(len, ref mut data) => {
                let byte_count = std::cmp::min(len - data.len(), buf.len());
                data.extend(&buf[..byte_count]);

                let request = bincode::deserialize_from(data.as_slice()).map_err(|_| Error::new(EINVAL))?;
                let response = crate::handle_channel_request(pci_state, tree, addr, request);

                let mut output_bytes = vec! [0_u8; 8];
                bincode::serialize_into(&mut output_bytes, &response).map_err(|_| Error::new(EIO))?;
                let len = output_bytes.len() - 8;
                output_bytes[..8].copy_from_slice(&u64::to_le_bytes(len as u64));
                *state = ChannelState::AwaitingResponseRead(output_bytes.into());

                Ok(byte_count)
            }
        }
    }
}
