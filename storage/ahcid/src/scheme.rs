use std::collections::BTreeMap;
use std::fmt::Write;
use std::str;

use common::io::Io as _;
use driver_block::{Disk, DiskWrapper};
use redox_scheme::{CallerCtx, OpenResult, SchemeBlockMut};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EINVAL, EISDIR, ENOENT, ENOLCK, EOVERFLOW, MODE_DIR,
    MODE_FILE, O_DIRECTORY, O_STAT,
};

use crate::ahci::hba::HbaMem;

#[derive(Clone)]
enum Handle {
    List(Vec<u8>),         // Dir contents buffer
    Disk(usize),           // Disk index
    Partition(usize, u32), // Disk index, partition index
}

pub struct DiskScheme {
    scheme_name: String,
    hba_mem: &'static mut HbaMem,
    disks: Box<[DiskWrapper]>,
    handles: BTreeMap<usize, Handle>,
    next_id: usize,
}

impl DiskScheme {
    pub fn new(
        scheme_name: String,
        hba_mem: &'static mut HbaMem,
        disks: Vec<Box<dyn Disk>>,
    ) -> DiskScheme {
        DiskScheme {
            scheme_name,
            hba_mem,
            disks: disks
                .into_iter()
                .map(DiskWrapper::new)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            handles: BTreeMap::new(),
            next_id: 0,
        }
    }

    pub fn irq(&mut self) -> bool {
        let is = self.hba_mem.is.read();
        if is > 0 {
            let pi = self.hba_mem.pi.read();
            let pi_is = pi & is;
            for i in 0..self.hba_mem.ports.len() {
                if pi_is & 1 << i > 0 {
                    let port = &mut self.hba_mem.ports[i];
                    let is = port.is.read();
                    port.is.write(is);
                }
            }
            self.hba_mem.is.write(is);
            true
        } else {
            false
        }
    }

    // Checks if any conflicting handles already exist
    fn check_locks(&self, disk_i: usize, part_i_opt: Option<u32>) -> Result<()> {
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

impl SchemeBlockMut for DiskScheme {
    fn xopen(&mut self, path: &str, flags: usize, ctx: &CallerCtx) -> Result<Option<OpenResult>> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path.trim_matches('/');

        let handle = if path_str.is_empty() {
            if flags & O_DIRECTORY != O_DIRECTORY && flags & O_STAT != O_STAT {
                return Err(Error::new(EISDIR));
            }
            let mut list = String::new();

            for (disk_index, disk) in self.disks.iter().enumerate() {
                write!(list, "{}\n", disk_index).unwrap();

                if disk.pt.is_none() {
                    continue;
                }
                for part_index in 0..disk.pt.as_ref().unwrap().partitions.len() {
                    write!(list, "{}p{}\n", disk_index, part_index).unwrap();
                }
            }

            Handle::List(list.into_bytes())
        } else if let Some(p_pos) = path_str.chars().position(|c| c == 'p') {
            let disk_id_str = &path_str[..p_pos];
            if p_pos + 1 >= path_str.len() {
                return Err(Error::new(ENOENT));
            }
            let part_id_str = &path_str[p_pos + 1..];
            let i = disk_id_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;
            let p = part_id_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            let disk = self.disks.get(i).ok_or(Error::new(ENOENT))?;
            if disk.pt.is_none()
                || disk
                    .pt
                    .as_ref()
                    .unwrap()
                    .partitions
                    .get(p as usize)
                    .is_none()
            {
                return Err(Error::new(ENOENT));
            }

            self.check_locks(i, Some(p))?;

            Handle::Partition(i, p)
        } else {
            let i = path_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;

            if self.disks.get(i).is_none() {
                return Err(Error::new(ENOENT));
            }
            self.check_locks(i, None)?;

            Handle::Disk(i)
        };
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, handle);
        Ok(Some(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        }))
    }

    fn xdup(&mut self, id: usize, buf: &[u8], _: &CallerCtx) -> Result<Option<OpenResult>> {
        if !buf.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let new_handle = {
            let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
            handle.clone()
        };

        let new_id = self.next_id;
        self.next_id += 1;
        self.handles.insert(new_id, new_handle);
        Ok(Some(OpenResult::ThisScheme {
            number: new_id,
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
                let disk = self.disks.get_mut(number).ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_size = disk.size();
                stat.st_blksize = disk.block_length()?;
                Ok(Some(0))
            }
            Handle::Partition(disk_id, part_num) => {
                let disk = self.disks.get_mut(disk_id).ok_or(Error::new(EBADF))?;
                let size = {
                    let pt = disk.pt.as_ref().ok_or(Error::new(EBADF))?;
                    let partition = pt
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?;
                    partition.size
                };

                stat.st_mode = MODE_FILE; // TODO: Block device?
                stat.st_size = size * u64::from(disk.block_length()?);
                stat.st_blksize = disk.block_length()?;
                stat.st_blocks = size;
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
                let path = format!("{}p{}", disk_num, part_num);
                let path_bytes = path.as_bytes();
                j = 0;
                while i < buf.len() && j < path_bytes.len() {
                    buf[i] = path_bytes[j];
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

                let byte_count = core::cmp::min(src.len(), buf.len());
                buf[..byte_count].copy_from_slice(&src[..byte_count]);
                Ok(Some(byte_count))
            }
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(number).ok_or(Error::new(EBADF))?;
                let blk_len = disk.block_length()?;
                disk.read(offset / u64::from(blk_len), buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(disk_num).ok_or(Error::new(EBADF))?;
                let blksize = disk.block_length()?;

                // validate that we're actually reading within the bounds of the partition
                let rel_block = offset / u64::from(blksize);

                let abs_block = {
                    let pt = disk.pt.as_ref().ok_or(Error::new(EBADF))?;
                    let partition = pt
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?;

                    let abs_block = partition.start_lba + rel_block;
                    // TODO: This shouldn't return EOVERFLOW?
                    if rel_block >= partition.size {
                        return Err(Error::new(EOVERFLOW));
                    }
                    abs_block
                };

                disk.read(abs_block, buf)
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
                let disk = self.disks.get_mut(number).ok_or(Error::new(EBADF))?;
                let blk_len = disk.block_length()?;
                disk.write(offset / u64::from(blk_len), buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(disk_num).ok_or(Error::new(EBADF))?;
                let blksize = disk.block_length()?;

                // validate that we're actually reading within the bounds of the partition
                let rel_block = offset / u64::from(blksize);

                let abs_block = {
                    let pt = disk.pt.as_ref().ok_or(Error::new(EBADF))?;
                    let partition = pt
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?;

                    let abs_block = partition.start_lba + rel_block;
                    if rel_block >= partition.size {
                        return Err(Error::new(EOVERFLOW));
                    }
                    abs_block
                };

                disk.write(abs_block, buf)
            }
        }
    }

    fn fsize(&mut self, id: usize) -> Result<Option<u64>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List(ref mut handle) => handle.len() as u64,
                Handle::Disk(number) => {
                    let disk = self.disks.get_mut(number).ok_or(Error::new(EBADF))?;
                    disk.size()
                }
                Handle::Partition(disk_num, part_num) => {
                    let disk = self.disks.get_mut(disk_num).ok_or(Error::new(EBADF))?;
                    let block_count = disk
                        .pt
                        .as_ref()
                        .ok_or(Error::new(EBADF))?
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?
                        .size;
                    u64::from(disk.block_length()?) * block_count
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
