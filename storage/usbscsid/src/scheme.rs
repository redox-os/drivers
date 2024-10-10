use std::collections::BTreeMap;
use std::{cmp, str};

use crate::protocol::Protocol;
use crate::scsi::Scsi;

use redox_scheme::{CallerCtx, OpenResult, SchemeMut};
use syscall::error::{Error, Result};
use syscall::error::{EACCES, EBADF, EINVAL, EIO, ENOENT, ENOSYS};
use syscall::flag::{MODE_CHR, MODE_DIR};
use syscall::flag::{O_DIRECTORY, O_STAT};
use syscall::flag::{SEEK_CUR, SEEK_END, SEEK_SET};
use syscall::schemev2::NewFdFlags;

// TODO: Only one disk, right?
const LIST_CONTENTS: &'static [u8] = b"0\n";

enum Handle {
    List,
    Disk,
    //Partition(usize, u32),
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

impl SchemeMut for ScsiScheme<'_> {
    fn xopen(&mut self, path_str: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path_str.trim_start_matches('/');
        let handle = if path_str.is_empty() {
            // List
            Handle::List
        } else if let Some(_p_pos) = path_str.chars().position(|c| c == 'p') {
            // TODO: Partitions.
            return Err(Error::new(ENOSYS));
        } else {
            Handle::Disk
        };
        self.next_fd += 1;
        self.handles.insert(self.next_fd, handle);
        Ok(OpenResult::ThisScheme {
            number: self.next_fd,
            flags: NewFdFlags::POSITIONED,
        })
    }
    fn fstat(&mut self, fd: usize, stat: &mut syscall::Stat) -> Result<usize> {
        match self.handles.get(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk => {
                stat.st_mode = MODE_CHR;
                stat.st_size = self.scsi.get_disk_size();
                stat.st_blksize = self.scsi.block_size;
                stat.st_blocks = self.scsi.block_count;
            }
            Handle::List => {
                stat.st_mode = MODE_DIR;
                stat.st_size = LIST_CONTENTS.len() as u64;
            }
        }
        Ok(0)
    }
    fn fpath(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        let path = match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk => "0",
            Handle::List => "",
        }
        .as_bytes();
        let min = std::cmp::min(path.len(), buf.len());
        buf[..min].copy_from_slice(&path[..min]);
        Ok(min)
    }
    fn fsize(&mut self, fd: usize) -> Result<u64> {
        Ok(match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk => self.scsi.get_disk_size(),
            Handle::List => LIST_CONTENTS.len() as u64,
        })
    }
    fn read(&mut self, fd: usize, buf: &mut [u8], offset: u64, _fcntl_flags: u32) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk => {
                if offset % u64::from(self.scsi.block_size) != 0
                    || buf.len() as u64 % u64::from(self.scsi.block_size) != 0
                {
                    return Err(Error::new(EINVAL));
                }
                let lba = offset / u64::from(self.scsi.block_size);
                match self.scsi.read(self.protocol, lba, buf) {
                    Ok(bytes_read) => Ok(bytes_read as usize),
                    Err(err) => {
                        eprintln!("usbscsid: READ IO ERROR: {err}");
                        Err(Error::new(EIO))
                    }
                }
            }
            Handle::List => {
                let src = usize::try_from(offset)
                    .ok()
                    .and_then(|o| LIST_CONTENTS.get(o..))
                    .unwrap_or(&[]);
                let min = core::cmp::min(src.len(), buf.len());
                buf[..min].copy_from_slice(&src[..min]);

                Ok(min)
            }
        }
    }
    fn write(&mut self, fd: usize, buf: &[u8], offset: u64, _fcntl_flags: u32) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Disk => {
                if offset % u64::from(self.scsi.block_size) != 0
                    || buf.len() as u64 % u64::from(self.scsi.block_size) != 0
                {
                    return Err(Error::new(EINVAL));
                }
                let lba = offset / u64::from(self.scsi.block_size);
                match self.scsi.write(self.protocol, lba, buf) {
                    Ok(bytes_written) => Ok(bytes_written as usize),
                    Err(err) => {
                        eprintln!("usbscsid: WRITE IO ERROR: {err}");
                        Err(Error::new(EIO))
                    }
                }
            }
            Handle::List => Err(Error::new(EBADF)),
        }
    }
}
