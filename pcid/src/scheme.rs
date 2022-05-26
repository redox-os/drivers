use std::collections::{BTreeMap, VecDeque};

use syscall::error::{Error, Result, EBADF, EINVAL, EIO, ENOENT, ESPIPE};
use syscall::scheme::SchemeMut;

use crate::{CfgAccess, PciAddr, State};
use crate::pci::{ConfigReader, ConfigWriter, PciFunc, PciDev, PciBus};

struct PciScheme {
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
}

enum ChannelState {
    AwaitingLenBytes(arrayvec::ArrayVec<u8, 8>),
    AwaitingData(usize, Vec<u8>),
    AwaitingResponseRead(VecDeque<u8>),
}

fn parse_pci_addr(addr: &str) -> Option<PciAddr> {
    let mut numbers = addr.split('.');

    Some(PciAddr {
        func: numbers.next_back().and_then(|n| n.parse::<u8>().ok())?,
        dev: numbers.next_back().and_then(|n| n.parse::<u8>().ok())?,
        bus: numbers.next_back().and_then(|n| n.parse::<u8>().ok())?,
        seg: numbers.next_back().unwrap_or("0").parse::<u16>().ok()?,
    })
}

const ROOT_CONTENTS: &[u8] = b"tree\n";
const DEVICE_CONTENTS: &[u8] = b"cfg-space\nchannel\n";

impl SchemeMut for PciScheme {
    fn open(&mut self, path: &str, flags: usize, uid: u32, gid: u32) -> Result<usize> {
        let expects_dir = path.ends_with('/');
        let path = path.trim_matches('/');

        let handle = if path.is_empty() {
            Handle::TopLevel { offset: 0 }
        } else if path.starts_with("tree") {
            let path = &path[4..];
            let index = path.find('/');
            Handle::Tree { offset: 0, bytes: Vec::new() }
        } else {
            return Err(Error::new(ENOENT))?;
        };

        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(id, handle);
        Ok(id)
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let (offset, bytes) = match *handle {
            Handle::CfgSpace { ref mut offset, addr } => return Self::read_cfgspace(&self.state, offset, addr, buf),
            Handle::TopLevel { ref mut offset } => (offset, ROOT_CONTENTS),
            Handle::Tree { ref mut offset, ref bytes } => (offset, bytes.as_slice()),
            Handle::Device { ref mut offset } => (offset, DEVICE_CONTENTS),
            Handle::Channel { addr, ref mut st } => return Self::read_channel(addr, st, buf),
        };

        let byte_count = core::cmp::min(DEVICE_CONTENTS.len().saturating_sub(*offset), buf.len());
        buf[..byte_count].copy_from_slice(&DEVICE_CONTENTS[*offset..]);
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

                let output = bincode::serialize(&response).map_err(|_| Error::new(EIO))?;
                *state = ChannelState::AwaitingResponseRead(output.into());

                Ok(byte_count)
            }
        }
    }
}
