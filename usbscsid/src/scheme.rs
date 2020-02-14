use std::collections::BTreeMap;
use std::{cmp, str};

use crate::protocol::Protocol;
use crate::scsi::Scsi;

use syscall::SchemeMut;
use syscall::error::{Error, Result};
use syscall::error::{EACCES, EBADF, EINVAL, EIO, ENOENT, ENOSYS};
use syscall::flag::{O_DIRECTORY, O_STAT};
use syscall::flag::{MODE_CHR, MODE_DIR};
use syscall::flag::{SEEK_CUR, SEEK_END, SEEK_SET};

// TODO: Only one disk, right?
const LIST_CONTENTS: &'static [u8] = b"0\n";

enum Handle {
    List(usize),
    Disk(usize),
    //Partition(usize, u32, usize),
}

pub struct ScsiScheme<'a> {
    scsi: &'a mut Scsi,
    protocol: &'a mut Protocol,
    handles: BTreeMap<usize, Handle>,
    next_fd: usize,
}

impl<'a> ScsiScheme<'a> {
    pub fn new(scsi: &'a mut Scsi, protocol: &'a mut dyn Protocol) -> Self {
        Self {
            scsi,
            protocol,
            handles: BTreeMap::new(),
            next_fd: 0,
        }
    }
}

impl<'a> SchemeMut for ScsiScheme<'a> {
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, gid: u32) -> Result<usize> {
        if uid != 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = str::from_utf8(path).or(Err(Error::new(ENOENT)))?.trim_start_matches('/');
        let handle = if path_str.is_empty() {
            // List
            Handle::List(0)
        } else if let Some(p_pos) = path_str.chars().position(|c| c == 'p') {
            // TODO: Partitions.
            return Err(Error::new(ENOSYS));
        } else {
            Handle::Disk(0)
        };
        self.next_fd += 1;
        self.handles.insert(self.next_fd, handle);
        Err(Error::new(ENOSYS))
    }
    fn fstat(&mut self, fd: usize, stat: &mut syscall::Stat) -> Result<usize> {
        match self.handles.get(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(_) => {
                stat.st_mode = MODE_CHR;
                stat.st_size = self.scsi.get_disk_size();
                stat.st_blksize = self.scsi.block_size;
                stat.st_blocks = self.scsi.block_count;
            }
            Handle::List(_) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = LIST_CONTENTS.len() as u64;
            }
        }
        Err(Error::new(ENOSYS))
    }
    fn fpath(&mut self, fd: usize, path: &mut [u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(ref mut offset) => {
                let len = self.scsi.get_disk_size() as usize;
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, len)),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, len)),
                    SEEK_END => cmp::max(0, cmp::min(len + pos, len)),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
            Handle::List(ref mut offset) => {
                let len = LIST_CONTENTS.len();
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, len)),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, len)),
                    SEEK_END => cmp::max(0, cmp::min(len + pos, len)),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
        }
    }
    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(ref mut offset) => {
                if *offset as u64 % u64::from(self.scsi.block_size) != 0 || buf.len() as u64 % u64::from(self.scsi.block_size) != 0 {
                    return Err(Error::new(EINVAL));
                }
                let lba = *offset as u64 / u64::from(self.scsi.block_size);
                let bytes_read = self.scsi.read(self.protocol, lba, buf).map_err(|err| dbg!(err)).or(Err(Error::new(EIO)))?;
                Ok(bytes_read as usize)
            }
            Handle::List(ref mut offset) => {
                let max_bytes_to_read = cmp::min(LIST_CONTENTS.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, *offset) - *offset;

                buf[..bytes_to_read].copy_from_slice(&LIST_CONTENTS[..bytes_to_read]);
                *offset += bytes_to_read;

                Ok(bytes_to_read)
            }
        }
    }
    fn write(&mut self, fd: usize, buf: &[u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
}
