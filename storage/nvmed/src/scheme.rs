use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::Write;
use std::io;
use std::io::prelude::*;
use std::sync::Arc;
use std::{cmp, str};

use redox_scheme::{CallerCtx, OpenResult, SchemeBlockMut};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EINVAL, EISDIR, ENOENT, ENOLCK, EOVERFLOW, MODE_DIR,
    MODE_FILE, O_DIRECTORY, O_STAT, SEEK_CUR, SEEK_END, SEEK_SET,
};

use crate::nvme::{Nvme, NvmeNamespace};

use partitionlib::{LogicalBlockSize, PartitionTable};

#[derive(Clone)]
enum Handle {
    List(Vec<u8>),       // entries
    Disk(u32),           // disk num
    Partition(u32, u32), // disk num, part num
}

pub struct DiskWrapper {
    inner: NvmeNamespace,
    pt: Option<PartitionTable>,
}

impl AsRef<NvmeNamespace> for DiskWrapper {
    fn as_ref(&self) -> &NvmeNamespace {
        &self.inner
    }
}

impl DiskWrapper {
    fn pt(disk: &mut NvmeNamespace, nvme: &Nvme) -> Option<PartitionTable> {
        let bs = match disk.block_size {
            512 => LogicalBlockSize::Lb512,
            4096 => LogicalBlockSize::Lb4096,
            _ => return None,
        };
        struct Device<'a, 'b> {
            disk: &'a mut NvmeNamespace,
            nvme: &'a Nvme,
            offset: u64,
            block_bytes: &'b mut [u8],
        }

        impl<'a, 'b> Seek for Device<'a, 'b> {
            fn seek(&mut self, from: io::SeekFrom) -> io::Result<u64> {
                let size_u = self.disk.blocks * self.disk.block_size;
                let size = i64::try_from(size_u).or(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Disk larger than 2^63 - 1 bytes",
                )))?;

                self.offset = match from {
                    io::SeekFrom::Start(new_pos) => cmp::min(size_u, new_pos),
                    io::SeekFrom::Current(new_pos) => {
                        cmp::max(0, cmp::min(size, self.offset as i64 + new_pos)) as u64
                    }
                    io::SeekFrom::End(new_pos) => {
                        cmp::max(0, cmp::min(size + new_pos, size)) as u64
                    }
                };

                Ok(self.offset)
            }
        }

        impl<'a, 'b> Read for Device<'a, 'b> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let blksize = self.disk.block_size;
                let size_in_blocks = self.disk.blocks;

                let disk = &mut self.disk;
                let nvme = &mut self.nvme;

                let read_block = |block: u64, block_bytes: &mut [u8]| {
                    if block >= size_in_blocks {
                        return Err(io::Error::from_raw_os_error(syscall::EOVERFLOW));
                    }
                    loop {
                        match nvme
                            .namespace_read(disk, disk.id, block, block_bytes)
                            .map_err(|err| io::Error::from_raw_os_error(err.errno))?
                        {
                            Some(bytes) => {
                                assert_eq!(bytes, block_bytes.len());
                                assert_eq!(bytes, blksize as usize);
                                return Ok(());
                            }
                            None => {
                                std::thread::yield_now();
                                continue;
                            } // TODO: Does this driver have (internal) error handling at all?
                        }
                    }
                };
                let bytes_read = driver_block::block_read(
                    self.offset,
                    blksize
                        .try_into()
                        .expect("Unreasonable block size above 2^32 bytes"),
                    buf,
                    self.block_bytes,
                    read_block,
                )?;
                self.offset += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        let mut block_bytes = [0u8; 4096];

        partitionlib::get_partitions(
            &mut Device {
                disk,
                nvme,
                offset: 0,
                block_bytes: &mut block_bytes[..bs.into()],
            },
            bs,
        )
        .ok()
        .flatten()
    }
    fn new(mut inner: NvmeNamespace, nvme: &Nvme) -> Self {
        Self {
            pt: Self::pt(&mut inner, nvme),
            inner,
        }
    }
}

pub struct DiskScheme {
    scheme_name: String,
    nvme: Arc<Nvme>,
    disks: BTreeMap<u32, DiskWrapper>,
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
                .map(|(k, v)| (k, DiskWrapper::new(v, &nvme)))
                .collect(),
            nvme,
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

impl SchemeBlockMut for DiskScheme {
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

    fn dup(&mut self, id: usize, buf: &[u8]) -> Result<Option<usize>> {
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
        Ok(Some(new_id))
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
                stat.st_blocks = disk.as_ref().blocks;
                stat.st_blksize = disk
                    .as_ref()
                    .block_size
                    .try_into()
                    .expect("Unreasonable block size of over 2^32 bytes");
                stat.st_size = disk.as_ref().blocks * disk.as_ref().block_size;
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
                stat.st_size = part.size * disk.as_ref().block_size;
                stat.st_blocks = part.size;
                stat.st_blksize = disk
                    .as_ref()
                    .block_size
                    .try_into()
                    .expect("Unreasonable block size of over 2^32 bytes");
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
                let block_size = disk.as_ref().block_size;
                self.nvme
                    .namespace_read(disk.as_ref(), disk.as_ref().id, offset / block_size, buf)
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

                let block_size = disk.as_ref().block_size;
                let rel_block = offset / block_size;
                if rel_block >= part.size {
                    return Err(Error::new(EOVERFLOW));
                }

                let abs_block = part.start_lba + rel_block;

                self.nvme
                    .namespace_read(disk.as_ref(), disk.as_ref().id, abs_block, buf)
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
                let block_size = disk.as_ref().block_size;
                self.nvme
                    .namespace_write(disk.as_ref(), disk.as_ref().id, offset / block_size, buf)
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

                let block_size = disk.as_ref().block_size;
                let rel_block = offset / block_size;
                if rel_block >= part.size {
                    return Err(Error::new(EOVERFLOW));
                }

                let abs_block = part.start_lba + rel_block;

                self.nvme
                    .namespace_write(disk.as_ref(), disk.as_ref().id, abs_block, buf)
            }
        }
    }

    fn fsize(&mut self, id: usize) -> Result<Option<u64>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List(ref handle) => handle.len() as u64,
                Handle::Disk(number) => {
                    let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                    disk.as_ref().blocks * disk.as_ref().block_size
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

                    part.size * disk.as_ref().block_size
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
