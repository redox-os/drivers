use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;

use syscall::error::{Error, Result, EBADF, EINVAL, EIO, ENOENT, ESPIPE};
use syscall::flag::{MODE_CHR, MODE_DIR, MODE_FILE};
use syscall::scheme::SchemeMut;

use crate::{CfgAccess, PciAddr, State};
use crate::pci::{ConfigReader, ConfigWriter, PciFunc, PciDev, PciBus};

pub struct PciScheme {
    handles: BTreeMap<usize, Handle>,
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
}

enum ChannelState {
    AwaitingLenBytes(arrayvec::ArrayVec<u8, 8>),
    AwaitingData(usize, Vec<u8>),
    AwaitingResponseRead(VecDeque<u8>),
}

const ROOT_CONTENTS: &[u8] = b"tree\n";
const DEVICE_CONTENTS: &[u8] = b"cfg-space\nchannel\n";

impl SchemeMut for PciScheme {
    fn open(&mut self, path: &str, flags: usize, uid: u32, gid: u32) -> Result<usize> {
        log::trace!("OPEN `{}` flags {}", path, flags);

        // TODO: Check flags are correct
        let expects_dir = path.ends_with('/');

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

        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(id, handle);
        Ok(id)
    }
    fn fstat(&mut self, id: usize, stat: &mut syscall::Stat) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        stat.st_mode = match handle {
            Handle::TopLevel { .. } | Handle::Tree { .. } | Handle::Device { .. } => MODE_DIR,
            Handle::CfgSpace { .. } | Handle::Channel { .. } => MODE_CHR,
            Handle::DeviceProperty { .. } => MODE_FILE,
        };
        Ok(0)
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let (offset, bytes) = match *handle {
            Handle::CfgSpace { ref mut offset, addr } => return Self::read_cfgspace(&self.state, offset, addr, buf),
            Handle::TopLevel { ref mut offset } => (offset, ROOT_CONTENTS),
            Handle::Tree { ref mut offset, ref bytes } => (offset, bytes.as_slice()),
            Handle::Device { ref mut offset } => (offset, DEVICE_CONTENTS),
            Handle::Channel { addr, ref mut st } => return Self::read_channel(addr, st, buf),
            Handle::DeviceProperty { ref mut offset, ref property } =>  (offset, property.as_bytes()),
        };

        let byte_count = core::cmp::min(bytes.len().saturating_sub(*offset), buf.len());
        buf[..byte_count].copy_from_slice(&bytes[*offset..*offset + byte_count]);
        *offset += byte_count;

        Ok(byte_count)
    }
    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        match *handle {
            Handle::CfgSpace { ref mut offset, addr } => Self::write_cfgspace(&self.state, offset, addr, buf),
            Handle::Channel { addr, ref mut st } => Self::write_channel(&self.state, &mut self.tree, addr, st, buf),

            _ => Err(Error::new(EBADF)),
        }
    }
    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let (offset, len) = match handle {
            Handle::Tree { offset, bytes, .. } => (offset, bytes.len()),
            Handle::TopLevel { offset } => (offset, ROOT_CONTENTS.len()),
            Handle::Device { offset } => (offset, DEVICE_CONTENTS.len()),
            Handle::CfgSpace { offset, .. } => (offset, Self::cfg_space_len(&self.state)),
            Handle::Channel { .. } => return Err(Error::new(ESPIPE)),
            Handle::DeviceProperty { offset, property } => (offset, property.len()),
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
                "irq" => p(func.header.interrupt_line().to_string()),
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
                _ => return None,
            }
        })
    }
    fn cfg_space_len(state: &State) -> usize {
        if state.pcie.is_some() { 4096 } else { 256 }
    }
    fn with_func<T>(addr: PciAddr, pci: &dyn CfgAccess, f: impl FnOnce(PciFunc) -> T) -> T {
        f(PciFunc { dev: &PciDev { bus: &PciBus { pci, num: addr.bus }, num: addr.dev }, num: addr.func })
    }
    fn read_cfgspace(state: &State, file_offset: &mut usize, addr: PciAddr, buf: &mut [u8]) -> Result<usize> {
        // TODO
        Err(Error::new(EIO))
    }
    fn write_cfgspace(state: &State, file_offset: &mut usize, addr: PciAddr, buf: &[u8]) -> Result<usize> {
        // TODO
        Err(Error::new(EIO))
    }

    fn read_channel(addr: PciAddr, state: &mut ChannelState, buf: &mut [u8]) -> Result<usize> {
        match *state {
            ChannelState::AwaitingResponseRead(ref mut queue) => {
                let byte_count = core::cmp::min(queue.len(), buf.len());
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
                let byte_count = core::cmp::min(len_bytes.capacity() - len_bytes.len(), buf.len());
                len_bytes.try_extend_from_slice(&buf[..byte_count]).unwrap();

                if let Ok(len_bytes) = len_bytes.clone().into_inner() {
                    // TODO: Validate length
                    let len = u64::from_le_bytes(len_bytes) as usize;
                    *state = ChannelState::AwaitingData(len, Vec::with_capacity(len));
                }

                Ok(byte_count)
            }
            ChannelState::AwaitingData(len, ref mut data) => {
                let byte_count = core::cmp::min(len - data.len(), buf.len());
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
