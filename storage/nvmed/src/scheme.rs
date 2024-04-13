use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::Write;
use std::io;
use std::io::prelude::*;
use std::sync::Arc;
use std::{cmp, str};

use redox_scheme::scheme::{LazyCallerCtx, Scheme};
use redox_scheme::{CallerCtx, OpenResult};
use syscall::{
    Error, Io, Result, Stat, EACCES, EBADF, EINVAL, EISDIR, ENOENT, ENOLCK,
    EOVERFLOW, MODE_DIR, MODE_FILE, O_DIRECTORY, O_STAT, SEEK_CUR, SEEK_END, SEEK_SET,
};

use crate::nvme::executor::LocalExecutor;
use crate::nvme::{Nvme, NvmeNamespace};

use partitionlib::{LogicalBlockSize, PartitionTable};

#[derive(Clone)]
enum Handle {
    List(Vec<u8>, usize),       // entries, offset
    Disk(u32, usize),           // disk num, offset
    Partition(u32, u32, usize), // disk num, part num, offset
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
            executor: &'a LocalExecutor,
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
                let executor = &self.executor;

                let read_block = |block: u64, block_bytes: &mut [u8]| {
                    if block >= size_in_blocks {
                        return Err(io::Error::from_raw_os_error(syscall::EOVERFLOW));
                    }
                    loop {
                        let bytes = executor.block_on(nvme.namespace_read(disk, disk.id, block, block_bytes))
                            .map_err(|err| io::Error::from_raw_os_error(err.errno))?;
                        assert_eq!(bytes, block_bytes.len());
                        assert_eq!(bytes, blksize as usize);
                        return Ok(());
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
                executor: &LocalExecutor::current(),
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
                Handle::Disk(i, _) => if disk_i == *i {
                    return Err(Error::new(ENOLCK));
                },
                Handle::Partition(i, p, _) => if disk_i == *i {
                    match part_i_opt {
                        Some(part_i) => if part_i == *p {
                            return Err(Error::new(ENOLCK));
                        },
                        None => {
                            return Err(Error::new(ENOLCK));
                        }
                    }
                },
                _ => (),
            }
        }
        Ok(())
    }
}

impl Scheme for DiskScheme {
    async fn open(&mut self, path_str: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path_str
            .trim_matches('/');
        if path_str.is_empty() {
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

                let id = self.next_id;
                self.next_id += 1;
                self.handles.insert(id, Handle::List(list.into_bytes(), 0));
                Ok(OpenResult::ThisScheme { number: id })
            } else {
                Err(Error::new(EISDIR))
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

                    let id = self.next_id;
                    self.next_id += 1;
                    self.handles
                        .insert(id, Handle::Partition(nsid, part_num, 0));
                    Ok(OpenResult::ThisScheme { number: id })
                } else {
                    Err(Error::new(ENOENT))
                }
            } else {
                Err(Error::new(ENOENT))
            }
        } else {
            let nsid = path_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            if self.disks.contains_key(&nsid) {
                self.check_locks(nsid, None)?;

                let id = self.next_id;
                self.next_id += 1;
                self.handles.insert(id, Handle::Disk(nsid, 0));
                Ok(OpenResult::ThisScheme { number: id })
            } else {
                Err(Error::new(ENOENT))
            }
        }
    }

    async fn dup(&mut self, id: usize, buf: &[u8], _ctx: &CallerCtx) -> Result<OpenResult> {
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
        Ok(OpenResult::ThisScheme { number: new_id })
    }

    async fn fstat(&mut self, id: usize, stat: &mut Stat, _ctx: &CallerCtx) -> Result<usize> {
        match *self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref data, _) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = data.len() as u64;
                Ok(0)
            }
            Handle::Disk(number, _) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_blocks = disk.as_ref().blocks;
                stat.st_blksize = disk
                    .as_ref()
                    .block_size
                    .try_into()
                    .expect("Unreasonable block size of over 2^32 bytes");
                stat.st_size = disk.as_ref().blocks * disk.as_ref().block_size;
                Ok(0)
            }
            Handle::Partition(disk_num, part_num, _) => {
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
                Ok(0)
            }
        }
    }

    async fn fpath(&mut self, id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
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
            Handle::Partition(disk_num, part_num, _) => {
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

        Ok(i)
    }

    async fn read(&mut self, id: usize, buf: &mut [u8], _ctx: &LazyCallerCtx) -> Result<usize> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let count = (&handle[*size..]).read(buf).unwrap();
                *size += count;
                Ok(count)
            }
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block_size = disk.as_ref().block_size;
                let count = self.nvme.namespace_read(disk.as_ref(), disk.as_ref().id, (*size as u64) / block_size, buf).await?;
                *size += count;
                Ok(count)
            }
            Handle::Partition(disk_num, part_num, ref mut offset) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let part = disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(EBADF))?
                    .partitions
                    .get(part_num as usize)
                    .ok_or(Error::new(EBADF))?;

                let block_size = disk.as_ref().block_size;
                let rel_block = (*offset as u64) / block_size;
                if rel_block >= part.size {
                    return Err(Error::new(EOVERFLOW));
                }

                let abs_block = part.start_lba + rel_block;

                let count = self.nvme.namespace_read(disk.as_ref(), disk.as_ref().id, abs_block, buf).await?;
                *offset += count;
                Ok(count)
            }
        }
    }

    async fn write(&mut self, id: usize, buf: &[u8], _ctx: &LazyCallerCtx) -> Result<usize> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(_, _) => Err(Error::new(EBADF)),
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block_size = disk.as_ref().block_size;
                let count = self.nvme.namespace_write(disk.as_ref(), disk.as_ref().id, (*size as u64) / block_size, buf).await?;
                *size += count;
                Ok(count)
            }
            Handle::Partition(disk_num, part_num, ref mut offset) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let part = disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(EBADF))?
                    .partitions
                    .get(part_num as usize)
                    .ok_or(Error::new(EBADF))?;

                let block_size = disk.as_ref().block_size;
                let rel_block = (*offset as u64) / block_size;
                if rel_block >= part.size {
                    return Err(Error::new(EOVERFLOW));
                }

                let abs_block = part.start_lba + rel_block;

                let count = self.nvme.namespace_write(disk.as_ref(), disk.as_ref().id, abs_block, buf).await?;
                *offset += count;
                Ok(count)
            }
        }
    }

    async fn seek(&mut self, id: usize, pos: isize, whence: usize, _ctx: &CallerCtx) -> Result<usize> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref mut handle, ref mut size) => {
                let len = handle.len() as isize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => {
                        cmp::max(0, cmp::min(len, *size as isize + pos))
                    }
                    SEEK_END => {
                        cmp::max(0, cmp::min(len, len + pos))
                    }
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(*size)
            }
            Handle::Disk(number, ref mut size) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let len = (disk.as_ref().blocks * disk.as_ref().block_size) as isize;
                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => {
                        cmp::max(0, cmp::min(len, *size as isize + pos))
                    }
                    SEEK_END => {
                        cmp::max(0, cmp::min(len, len + pos))
                    }
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(*size)
            }
            Handle::Partition(disk_num, part_num, ref mut size) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let part = disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(EBADF))?
                    .partitions
                    .get(part_num as usize)
                    .ok_or(Error::new(EBADF))?;

                let len = (part.size * disk.as_ref().block_size) as isize;

                *size = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => {
                        cmp::max(0, cmp::min(len, *size as isize + pos))
                    }
                    SEEK_END => {
                        cmp::max(0, cmp::min(len, len + pos))
                    }
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(*size)
            }
        }
    }

    async fn close(&mut self, id: usize) -> Result<()> {
        self.handles
            .remove(&id)
            .ok_or(Error::new(EBADF))?;
        Ok(())
    }
}
