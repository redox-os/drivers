use std::collections::BTreeMap;
use std::{cmp, str};
use std::fmt::Write;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use syscall::{Error, EACCES, EBADF, EINVAL, EISDIR, ENOENT, Result, Scheme, Stat, MODE_DIR, MODE_FILE, O_DIRECTORY, O_STAT, SEEK_CUR, SEEK_END, SEEK_SET};

use ahci::Disk;

#[derive(Clone)]
enum Handle {
    List(Vec<u8>, usize),
    Disk(Arc<Mutex<Box<Disk>>>, usize, usize)
}

pub struct DiskScheme {
    scheme_name: String,
    disks: Box<[Arc<Mutex<Box<Disk>>>]>,
    handles: Mutex<BTreeMap<usize, Handle>>,
    next_id: AtomicUsize
}

impl DiskScheme {
    pub fn new(scheme_name: String, disks: Vec<Box<Disk>>) -> DiskScheme {
        let mut disk_arcs = vec![];
        for disk in disks {
            disk_arcs.push(Arc::new(Mutex::new(disk)));
        }

        DiskScheme {
            scheme_name: scheme_name,
            disks: disk_arcs.into_boxed_slice(),
            handles: Mutex::new(BTreeMap::new()),
            next_id: AtomicUsize::new(0)
        }
    }
}

impl Scheme for DiskScheme {
    fn open(&self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid == 0 {
            let path_str = str::from_utf8(path).or(Err(Error::new(ENOENT)))?.trim_matches('/');
            if path_str.is_empty() {
                if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                    let mut list = String::new();

                    for i in 0..self.disks.len() {
                        write!(list, "{}\n", i).unwrap();
                    }

                    let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                    self.handles.lock().insert(id, Handle::List(list.into_bytes(), 0));
                    Ok(id)
                } else {
                    Err(Error::new(EISDIR))
                }
            } else {
                let i = path_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;

                if let Some(disk) = self.disks.get(i) {
                    let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                    self.handles.lock().insert(id, Handle::Disk(disk.clone(), 0, i));
                    Ok(id)
                } else {
                    Err(Error::new(ENOENT))
                }
            }
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn dup(&self, id: usize, buf: &[u8]) -> Result<usize> {
        if ! buf.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let mut handles = self.handles.lock();
        let new_handle = {
            let handle = handles.get(&id).ok_or(Error::new(EBADF))?;
            handle.clone()
        };

        let new_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        handles.insert(new_id, new_handle);
        Ok(new_id)
    }

    fn fstat(&self, id: usize, stat: &mut Stat) -> Result<usize> {
        let handles = self.handles.lock();
        match *handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref handle, _) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = handle.len() as u64;
                Ok(0)
            },
            Handle::Disk(ref handle, _, _) => {
                stat.st_mode = MODE_FILE;
                stat.st_size = handle.lock().size();
                Ok(0)
            }
        }
    }

    fn fpath(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handles = self.handles.lock();
        let handle = handles.get(&id).ok_or(Error::new(EBADF))?;

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
            Handle::Disk(_, _, number) => {
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

        Ok(i)
    }

    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let mut handles = self.handles.lock();
        match *handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let count = (&handle[*size..]).read(buf).unwrap();
                *size += count;
                Ok(count)
            },
            Handle::Disk(ref mut handle, ref mut size, _) => {
                let mut disk = handle.lock();
                let count = disk.read((*size as u64)/512, buf)?;
                *size += count;
                Ok(count)
            }
        }
    }

    fn write(&self, id: usize, buf: &[u8]) -> Result<usize> {
        let mut handles = self.handles.lock();
        match *handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(_, _) => {
                Err(Error::new(EBADF))
            },
            Handle::Disk(ref mut handle, ref mut size, _) => {
                let mut disk = handle.lock();
                let count = disk.write((*size as u64)/512, buf)?;
                *size += count;
                Ok(count)
            }
        }
    }

    fn seek(&self, id: usize, pos: usize, whence: usize) -> Result<usize> {
        let mut handles = self.handles.lock();
        match *handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let len = handle.len() as usize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len as isize, *size as isize + pos as isize)) as usize,
                    SEEK_END => cmp::max(0, cmp::min(len as isize, len as isize + pos as isize)) as usize,
                    _ => return Err(Error::new(EINVAL))
                };

                Ok(*size)
            },
            Handle::Disk(ref mut handle, ref mut size, _) => {
                let len = handle.lock().size() as usize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len as isize, *size as isize + pos as isize)) as usize,
                    SEEK_END => cmp::max(0, cmp::min(len as isize, len as isize + pos as isize)) as usize,
                    _ => return Err(Error::new(EINVAL))
                };

                Ok(*size)
            }
        }
    }

    fn close(&self, id: usize) -> Result<usize> {
        let mut handles = self.handles.lock();
        handles.remove(&id).ok_or(Error::new(EBADF)).and(Ok(0))
    }
}
