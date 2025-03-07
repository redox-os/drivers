use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::Write;
use std::str;
use std::sync::Arc;

use driver_block::{Disk, DiskWrapper};
use redox_scheme::{CallerCtx, OpenResult, SchemeBlock};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EISDIR, ENOENT, ENOLCK, MODE_DIR, MODE_FILE, O_DIRECTORY,
    O_STAT,
};

use crate::nvme::{Nvme, NvmeNamespace};

enum Handle {
    List(Vec<u8>),       // entries
    Disk(u32),           // disk num
    Partition(u32, u32), // disk num, part num
}

struct NvmeDisk(Arc<Nvme>, NvmeNamespace);

impl Disk for NvmeDisk {
    fn block_size(&self) -> u32 {
        self.1.block_size.try_into().unwrap()
    }

    fn size(&self) -> u64 {
        self.1.blocks * self.1.block_size
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<Option<usize>> {
        self.0.namespace_read(self.1, block, buffer)
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<Option<usize>> {
        self.0.namespace_write(self.1, block, buffer)
    }
}

pub struct DiskScheme {
    scheme_name: String,
    disks: BTreeMap<u32, DiskWrapper<NvmeDisk>>,
    handles: BTreeMap<usize, Handle>,
    next_id: usize,
}

impl DiskScheme {
    pub fn new(
        scheme_name: String,
        nvme: Arc<Nvme>,
        disks: BTreeMap<u32, NvmeNamespace>,
    ) -> DiskScheme {
        DiskScheme {
            scheme_name,
            disks: disks
                .into_iter()
                .map(|(k, ns)| (k, DiskWrapper::new(NvmeDisk(nvme.clone(), ns))))
                .collect(),
            handles: BTreeMap::new(),
            next_id: 0,
        }
    }

    // Checks if any conflicting handles already exist
    fn check_locks(&self, disk_i: u32, part_i_opt: Option<u32>) -> Result<()> {
        for (_, handle) in self.handles.iter() {
            match handle {
                Handle::Disk(i) => {
                    if disk_i == *i {
                        return Err(Error::new(ENOLCK));
                    }
                }
                Handle::Partition(i, p) => {
                    if disk_i == *i {
                        match part_i_opt {
                            Some(part_i) => {
                                if part_i == *p {
                                    return Err(Error::new(ENOLCK));
                                }
                            }
                            None => {
                                return Err(Error::new(ENOLCK));
                            }
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
}

impl SchemeBlock for DiskScheme {
    fn xopen(
        &mut self,
        path_str: &str,
        flags: usize,
        ctx: &CallerCtx,
    ) -> Result<Option<OpenResult>> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path_str.trim_matches('/');

        let handle = if path_str.is_empty() {
            if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                let mut list = String::new();

                for (nsid, disk) in self.disks.iter() {
                    write!(list, "{}\n", nsid).unwrap();

                    if disk.pt.is_none() {
                        continue;
                    }
                    for part_num in 0..disk.pt.as_ref().unwrap().partitions.len() {
                        write!(list, "{}p{}\n", nsid, part_num).unwrap();
                    }
                }

                Handle::List(list.into_bytes())
            } else {
                return Err(Error::new(EISDIR));
            }
        } else if let Some(p_pos) = path_str.chars().position(|c| c == 'p') {
            let nsid_str = &path_str[..p_pos];

            if p_pos + 1 >= path_str.len() {
                return Err(Error::new(ENOENT));
            }
            let part_num_str = &path_str[p_pos + 1..];

            let nsid = nsid_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;
            let part_num = part_num_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            if let Some(disk) = self.disks.get(&nsid) {
                if disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(ENOENT))?
                    .partitions
                    .get(part_num as usize)
                    .is_some()
                {
                    self.check_locks(nsid, Some(part_num))?;

                    Handle::Partition(nsid, part_num)
                } else {
                    return Err(Error::new(ENOENT));
                }
            } else {
                return Err(Error::new(ENOENT));
            }
        } else {
            let nsid = path_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            if self.disks.contains_key(&nsid) {
                self.check_locks(nsid, None)?;
                Handle::Disk(nsid)
            } else {
                return Err(Error::new(ENOENT));
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, handle);
        Ok(Some(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        }))
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<Option<usize>> {
        match *self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref data) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = data.len() as u64;
                Ok(Some(0))
            }
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_blocks = disk.disk().1.blocks;
                stat.st_blksize = disk.block_size();
                stat.st_size = disk.size();
                Ok(Some(0))
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let part = disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(EBADF))?
                    .partitions
                    .get(part_num as usize)
                    .ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_size = part.size * u64::from(disk.block_size());
                stat.st_blocks = part.size;
                stat.st_blksize = disk.block_size();
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
            Handle::List(_) => (),
            Handle::Disk(number) => {
                let number_str = format!("{}", number);
                let number_bytes = number_str.as_bytes();
                j = 0;
                while i < buf.len() && j < number_bytes.len() {
                    buf[i] = number_bytes[j];
                    i += 1;
                    j += 1;
                }
            }
            Handle::Partition(disk_num, part_num) => {
                let number_str = format!("{}p{}", disk_num, part_num);
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

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref handle) => {
                let src = usize::try_from(offset)
                    .ok()
                    .and_then(|o| handle.get(o..))
                    .unwrap_or(&[]);
                let count = core::cmp::min(src.len(), buf.len());
                buf[..count].copy_from_slice(&src[..count]);
                Ok(Some(count))
            }
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.read(None, block, buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.read(Some(part_num as usize), block, buf)
            }
        }
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(_) => Err(Error::new(EBADF)),
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.write(None, block, buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.write(Some(part_num as usize), block, buf)
            }
        }
    }

    fn fsize(&mut self, id: usize) -> Result<Option<u64>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List(ref handle) => handle.len() as u64,
                Handle::Disk(number) => {
                    let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                    disk.size()
                }
                Handle::Partition(disk_num, part_num) => {
                    let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                    let part = disk
                        .pt
                        .as_ref()
                        .ok_or(Error::new(EBADF))?
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?;

                    part.size * u64::from(disk.block_size())
                }
            },
        ))
    }

    fn close(&mut self, id: usize) -> Result<Option<usize>> {
        self.handles
            .remove(&id)
            .ok_or(Error::new(EBADF))
            .and(Ok(Some(0)))
    }
}
