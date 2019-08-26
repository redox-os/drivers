use std::collections::BTreeMap;
use std::{cmp, str};
use std::fmt::Write;
use std::io::Read;
use syscall::{
    Error, EACCES, EBADF, EINVAL, EISDIR, ENOENT, Result,
    Io, SchemeBlockMut, Stat, MODE_DIR, MODE_FILE, O_DIRECTORY,
    O_STAT, SEEK_CUR, SEEK_END, SEEK_SET};

use crate::nvme::{Nvme, NvmeNamespace};

#[derive(Clone)]
enum Handle {
    List(Vec<u8>, usize),
    Disk(u32, usize)
}

pub struct DiskScheme {
    scheme_name: String,
    nvme: Nvme,
    disks: BTreeMap<u32, NvmeNamespace>,
    handles: BTreeMap<usize, Handle>,
    next_id: usize
}

impl DiskScheme {
    pub fn new(scheme_name: String, nvme: Nvme, disks: BTreeMap<u32, NvmeNamespace>) -> DiskScheme {
        DiskScheme {
            scheme_name,
            nvme,
            disks,
            handles: BTreeMap::new(),
            next_id: 0
        }
    }
}

impl DiskScheme {
    pub fn irq(&mut self) -> bool {
        // TODO: implement
        false
    }
}

impl SchemeBlockMut for DiskScheme {
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<Option<usize>> {
        if uid == 0 {
            let path_str = str::from_utf8(path).or(Err(Error::new(ENOENT)))?.trim_matches('/');
            if path_str.is_empty() {
                if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                    let mut list = String::new();

                    for (nsid, _disk) in self.disks.iter() {
                        write!(list, "{}\n", nsid).unwrap();
                    }

                    let id = self.next_id;
                    self.next_id += 1;
                    self.handles.insert(id, Handle::List(list.into_bytes(), 0));
                    Ok(Some(id))
                } else {
                    Err(Error::new(EISDIR))
                }
            } else {
                let nsid = path_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

                if self.disks.contains_key(&nsid) {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.handles.insert(id, Handle::Disk(nsid, 0));
                    Ok(Some(id))
                } else {
                    Err(Error::new(ENOENT))
                }
            }
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn dup(&mut self, id: usize, buf: &[u8]) -> Result<Option<usize>> {
        if ! buf.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let new_handle = {
            let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
            handle.clone()
        };

        let new_id = self.next_id;
        self.next_id += 1;
        self.handles.insert(new_id, new_handle);
        Ok(Some(new_id))
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<Option<usize>> {
        match *self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref data, _) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = data.len() as u64;
                Ok(Some(0))
            },
            Handle::Disk(number, _) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_size = disk.blocks * disk.block_size;
                Ok(Some(0))
            }
        }
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;

        let scheme_name = self.scheme_name.as_bytes();
        let mut j = 0;
        while i < buf.len() && j < scheme_name.len() {
            buf[i] = scheme_name[j];
            i += 1;
            j += 1;
        }

        if i < buf.len() {
            buf[i] = b':';
            i += 1;
        }

        match *handle {
            Handle::List(_, _) => (),
            Handle::Disk(number, _) => {
                let number_str = format!("{}", number);
                let number_bytes = number_str.as_bytes();
                j = 0;
                while i < buf.len() && j < number_bytes.len() {
                    buf[i] = number_bytes[j];
                    i += 1;
                    j += 1;
                }
            }
        }

        Ok(Some(i))
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let count = (&handle[*size..]).read(buf).unwrap();
                *size += count;
                Ok(Some(count))
            },
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block_size = disk.block_size;
                if let Some(count) = unsafe {
                    self.nvme.namespace_read(disk.id, (*size as u64)/block_size, buf)?
                } {
                    *size += count;
                    Ok(Some(count))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(_, _) => {
                Err(Error::new(EBADF))
            },
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block_size = disk.block_size;
                if let Some(count) = unsafe {
                    self.nvme.namespace_write(disk.id, (*size as u64)/block_size, buf)?
                } {
                    *size += count;
                    Ok(Some(count))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn seek(&mut self, id: usize, pos: usize, whence: usize) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let len = handle.len() as usize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len as isize, *size as isize + pos as isize)) as usize,
                    SEEK_END => cmp::max(0, cmp::min(len as isize, len as isize + pos as isize)) as usize,
                    _ => return Err(Error::new(EINVAL))
                };

                Ok(Some(*size))
            },
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let len = (disk.blocks * disk.block_size) as usize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len as isize, *size as isize + pos as isize)) as usize,
                    SEEK_END => cmp::max(0, cmp::min(len as isize, len as isize + pos as isize)) as usize,
                    _ => return Err(Error::new(EINVAL))
                };

                Ok(Some(*size))
            }
        }
    }

    fn close(&mut self, id: usize) -> Result<Option<usize>> {
        self.handles.remove(&id).ok_or(Error::new(EBADF)).and(Ok(Some(0)))
    }
}
