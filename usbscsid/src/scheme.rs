use std::collections::BTreeMap;
use std::{cmp, str};

use crate::protocol::Protocol;
use crate::scsi::Scsi;

use syscall::error::{Error, Result};
use syscall::error::{EACCES, EBADF, EINVAL, EIO, ENOENT, ENOSYS};
use syscall::flag::{MODE_CHR, MODE_DIR};
use syscall::flag::{O_DIRECTORY, O_STAT};
use syscall::flag::{SEEK_CUR, SEEK_END, SEEK_SET};
use syscall::SchemeMut;

// TODO: Only one disk, right?
const LIST_CONTENTS: &'static [u8] = b"0\n";

enum Handle {
    List(usize),
    Disk(usize),
    //Partition(usize, u32, usize),
}

pub struct ScsiScheme<'a> {
    scsi: &'a mut Scsi,
    protocol: &'a mut dyn Protocol,
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
    fn open(&mut self, path_str: &str, flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid != 0 {
            return Err(Error::new(EACCES));
        }
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path_str
            .trim_start_matches('/');
        let handle = if path_str.is_empty() {
            // List
            Handle::List(0)
        } else if let Some(_p_pos) = path_str.chars().position(|c| c == 'p') {
            // TODO: Partitions.
            return Err(Error::new(ENOSYS));
        } else {
            Handle::Disk(0)
        };
        self.next_fd += 1;
        self.handles.insert(self.next_fd, handle);
        Ok(self.next_fd)
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
        Ok(0)
    }
    fn fpath(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        let path = match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(_) => "0",
            Handle::List(_) => "",
        }
        .as_bytes();
        let min = std::cmp::min(path.len(), buf.len());
        buf[..min].copy_from_slice(&path[..min]);
        Ok(min)
    }
    fn seek(&mut self, fd: usize, pos: isize, whence: usize) -> Result<isize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(ref mut offset) => {
                let len = self.scsi.get_disk_size() as isize;
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, len)),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset as isize + pos, len)),
                    SEEK_END => cmp::max(0, cmp::min(len + pos, len)),
                    _ => return Err(Error::new(EINVAL)),
                } as usize;
                Ok(*offset as isize)
            }
            Handle::List(ref mut offset) => {
                let len = LIST_CONTENTS.len() as isize;
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, len)),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset as isize + pos, len)),
                    SEEK_END => cmp::max(0, cmp::min(len + pos, len)),
                    _ => return Err(Error::new(EINVAL)),
                } as usize;
                Ok(*offset as isize)
            }
        }
    }
    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(ref mut offset) => {
                if *offset as u64 % u64::from(self.scsi.block_size) != 0
                    || buf.len() as u64 % u64::from(self.scsi.block_size) != 0
                {
                    return Err(Error::new(EINVAL));
                }
                let lba = *offset as u64 / u64::from(self.scsi.block_size);
                let bytes_read = self
                    .scsi
                    .read(self.protocol, lba, buf)
                    .map_err(|err| dbg!(err))
                    .or(Err(Error::new(EIO)))?;
                *offset += bytes_read as usize;
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
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk(ref mut offset) => {
                if *offset as u64 % u64::from(self.scsi.block_size) != 0
                    || buf.len() as u64 % u64::from(self.scsi.block_size) != 0
                {
                    return Err(Error::new(EINVAL));
                }
                let lba = *offset as u64 / u64::from(self.scsi.block_size);
                let bytes_written = self
                    .scsi
                    .write(self.protocol, lba, buf)
                    .map_err(|err| dbg!(err))
                    .or(Err(Error::new(EIO)))?;
                *offset += bytes_written as usize;
                Ok(bytes_written as usize)
            }
            Handle::List(_) => Err(Error::new(EBADF)),
        }
    }
}
