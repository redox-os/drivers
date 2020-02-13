use std::collections::BTreeMap;
use std::str;

use crate::scsi::Scsi;

use syscall::SchemeMut;
use syscall::error::{Error, Result};
use syscall::error::{EACCES, EBADF, ENOENT, ENOSYS};
use syscall::flag::{O_DIRECTORY, O_STAT};
use syscall::flag::{MODE_CHR, MODE_DIR};

// TODO: Only one disk, right?
const LIST_CONTENTS: &'static [u8] = b"disk\n";

enum Handle {
    List(usize),
    Disk(usize),
    //Partition(usize, u32, usize),
}

pub struct ScsiScheme<'a> {
    scsi: &'a mut Scsi,
    handles: BTreeMap<usize, Handle>,
    next_fd: usize,
}

impl<'a> ScsiScheme<'a> {
    pub fn new(scsi: &'a mut Scsi) -> Self {
        Self {
            scsi,
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
                // TODO: stat.st_blocks
            }
            Handle::List(_) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = LIST_CONTENTS.len() as u64;
                // TODO: stat.st_blocks
            }
        }
        Err(Error::new(ENOSYS))
    }
    fn fpath(&mut self, fd: usize, path: &mut [u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
    fn write(&mut self, fd: usize, buf: &[u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }
}
