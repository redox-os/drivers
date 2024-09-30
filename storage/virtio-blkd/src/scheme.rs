use std::cmp;
use std::collections::BTreeMap;
use std::io::Read;
use std::io::Result as IoResult;
use std::io::Seek;

use std::fmt::Write;
use std::sync::Arc;

use common::dma::Dma;
use partitionlib::LogicalBlockSize;
use partitionlib::PartitionTable;

use redox_scheme::CallerCtx;
use redox_scheme::OpenResult;
use redox_scheme::SchemeBlockMut;
use syscall::error::*;
use syscall::flag::*;
use syscall::schemev2::NewFdFlags;
use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::Queue;

use crate::BlockDeviceConfig;
use crate::BlockRequestTy;
use crate::BlockVirtRequest;

const BLK_SIZE: u64 = 512;

trait BlkExtension {
    async fn read(&self, block: u64, target: &mut [u8]) -> usize;
    async fn write(&self, block: u64, target: &[u8]) -> usize;
}

impl BlkExtension for Queue<'_> {
    async fn read(&self, block: u64, target: &mut [u8]) -> usize {
        let req = Dma::new(BlockVirtRequest {
            ty: BlockRequestTy::In,
            reserved: 0,
            sector: block,
        })
        .unwrap();

        let result = unsafe {
            Dma::<[u8]>::zeroed_slice(target.len())
                .unwrap()
                .assume_init()
        };
        let status = Dma::new(u8::MAX).unwrap();

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&req))
            .chain(Buffer::new_unsized(&result).flags(DescriptorFlags::WRITE_ONLY))
            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        // XXX: Subtract 1 because the of status byte.
        let written = self.send(chain).await as usize - 1;
        assert_eq!(*status, 0);

        target[..written].copy_from_slice(&result);
        written
    }

    async fn write(&self, block: u64, target: &[u8]) -> usize {
        let req = Dma::new(BlockVirtRequest {
            ty: BlockRequestTy::Out,
            reserved: 0,
            sector: block,
        })
        .unwrap();

        let mut result = unsafe {
            Dma::<[u8]>::zeroed_slice(target.len())
                .unwrap()
                .assume_init()
        };
        result.copy_from_slice(target.as_ref());

        let status = Dma::new(u8::MAX).unwrap();

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&req))
            .chain(Buffer::new_sized(&result, target.len()))
            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.send(chain).await as usize;
        assert_eq!(*status, 0);

        target.len()
    }
}

pub enum Handle {
    Partition {
        /// Partition Number
        number: u32,
    },

    List {
        entries: Vec<u8>,
    },

    Disk,
}

pub struct DiskScheme<'a> {
    queue: Arc<Queue<'a>>,
    next_id: usize,
    cfg: BlockDeviceConfig,
    handles: BTreeMap<usize, Handle>,
    part_table: Option<PartitionTable>,
}

impl<'a> DiskScheme<'a> {
    pub fn new(queue: Arc<Queue<'a>>, cfg: BlockDeviceConfig) -> Self {
        let mut this = Self {
            queue,
            next_id: 0,
            cfg,
            handles: BTreeMap::new(),
            part_table: None,
        };

        struct VirtioShim<'a, 'b> {
            scheme: &'b DiskScheme<'a>,
            offset: u64,
            block_bytes: &'b mut [u8],
        }

        impl<'a, 'b> Read for VirtioShim<'a, 'b> {
            fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
                let read_block =
                    |block: u64, block_bytes: &mut [u8]| -> Result<(), std::io::Error> {
                        let req = Dma::new(BlockVirtRequest {
                            ty: BlockRequestTy::In,
                            reserved: 0,
                            sector: block,
                        })
                        .unwrap();

                        let result = Dma::new([0u8; 512]).unwrap();
                        let status = Dma::new(u8::MAX).unwrap();

                        let chain = ChainBuilder::new()
                            .chain(Buffer::new(&req))
                            .chain(Buffer::new(&result).flags(DescriptorFlags::WRITE_ONLY))
                            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
                            .build();

                        futures::executor::block_on(self.scheme.queue.send(chain));
                        assert_eq!(*status, 0);

                        let size = core::cmp::min(block_bytes.len(), result.len());
                        block_bytes[..size].copy_from_slice(&result.as_slice()[..size]);
                        Ok(())
                    };

                let bytes_read =
                    driver_block::block_read(self.offset, 512, buf, self.block_bytes, read_block)
                        .unwrap();
                self.offset += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        impl<'a, 'b> Seek for VirtioShim<'a, 'b> {
            fn seek(&mut self, from: std::io::SeekFrom) -> IoResult<u64> {
                let size_u = self.scheme.cfg.capacity() * self.scheme.cfg.block_size() as u64;
                let size = i64::try_from(size_u).or(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Disk larger than 2^63 - 1 bytes",
                )))?;

                self.offset = match from {
                    std::io::SeekFrom::Start(new_pos) => std::cmp::min(size_u, new_pos),
                    std::io::SeekFrom::Current(new_pos) => {
                        std::cmp::max(0, std::cmp::min(size, self.offset as i64 + new_pos)) as u64
                    }
                    std::io::SeekFrom::End(new_pos) => {
                        std::cmp::max(0, std::cmp::min(size + new_pos, size)) as u64
                    }
                };

                Ok(self.offset)
            }
        }

        let mut shim = VirtioShim {
            scheme: &this,
            offset: 0,
            block_bytes: &mut [0u8; 4096],
        };

        let part_table = partitionlib::get_partitions(&mut shim, LogicalBlockSize::Lb512)
            .ok()
            .flatten();

        this.part_table = part_table;
        this
    }
}

impl<'a> SchemeBlockMut for DiskScheme<'a> {
    fn xopen(
        &mut self,
        path: &str,
        flags: usize,
        _ctx: &CallerCtx,
    ) -> syscall::Result<Option<OpenResult>> {
        log::info!("virtiod: open: {}", path);

        let path_str = path.trim_matches('/');
        if path_str.is_empty() {
            if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                let mut list = String::new();
                // FIXME: The zero is the disk identifier (look in the nvmed scheme, it set's this
                //            to the namespace id).
                write!(list, "{}\n", 0).unwrap();

                if let Some(part_table) = &self.part_table {
                    for part_num in 0..part_table.partitions.len() {
                        write!(list, "{}p{}\n", 0, part_num).unwrap();
                    }
                }

                let id = self.next_id;
                self.next_id += 1;
                self.handles.insert(
                    id,
                    Handle::List {
                        entries: list.into_bytes(),
                    },
                );

                Ok(Some(OpenResult::ThisScheme {
                    number: id,
                    flags: NewFdFlags::POSITIONED,
                }))
            } else {
                return Err(syscall::Error::new(EISDIR));
            }
        } else if let Some(p_pos) = path_str.chars().position(|c| c == 'p') {
            let _nsid_str = &path_str[..p_pos];

            if p_pos + 1 >= path_str.len() {
                return Err(Error::new(ENOENT));
            }
            let part_num_str = &path_str[p_pos + 1..];
            let part_num = part_num_str.parse::<u32>().unwrap();

            let part_table = self.part_table.as_ref().unwrap();
            let _part = part_table.partitions.get(part_num as usize).unwrap();

            let id = self.next_id;
            self.next_id += 1;
            self.handles
                .insert(id, Handle::Partition { number: part_num });

            Ok(Some(OpenResult::ThisScheme {
                number: id,
                flags: NewFdFlags::POSITIONED,
            }))
        } else {
            let nsid = path_str.parse::<u32>().unwrap();
            assert_eq!(nsid, 0);

            let id = self.next_id;
            self.next_id += 1;
            self.handles.insert(id, Handle::Disk);
            Ok(Some(OpenResult::ThisScheme {
                number: id,
                flags: NewFdFlags::POSITIONED,
            }))
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<Option<usize>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List { ref mut entries } => {
                    let src = usize::try_from(offset)
                        .ok()
                        .and_then(|o| entries.get(o..))
                        .unwrap_or(&[]);
                    let count = core::cmp::min(src.len(), buf.len());
                    buf[..count].copy_from_slice(&src[..count]);
                    count
                }

                Handle::Partition { number } => {
                    let part_table = self.part_table.as_ref().unwrap();
                    let part = part_table
                        .partitions
                        .get(number as usize)
                        .ok_or(Error::new(EBADF))?;

                    // Get the offset in sectors.
                    let rel_block = offset / BLK_SIZE;
                    // if rel_block >= part.size {
                    //     return Err(Error::new(EOVERFLOW));
                    // }

                    let abs_block = part.start_lba + rel_block;

                    futures::executor::block_on(self.queue.read(abs_block, buf))
                }

                Handle::Disk => {
                    let block_size = self.cfg.block_size();

                    futures::executor::block_on(
                        self.queue.read(offset / u64::from(block_size), buf),
                    )
                }
            },
        ))
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<Option<usize>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::Disk => {
                    let block_size = self.cfg.block_size();
                    futures::executor::block_on(
                        self.queue.write(offset / u64::from(block_size), buf),
                    )
                }

                _ => todo!(),
            },
        ))
    }

    fn fsize(&mut self, id: usize) -> syscall::Result<Option<u64>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List { ref entries } => {
                    let len = entries.len() as u64;
                    log::debug!("list: part_len={len:?}");

                    len
                }

                Handle::Partition { number } => {
                    let part_table = self.part_table.as_ref().unwrap();
                    let part = part_table
                        .partitions
                        .get(number as usize)
                        .ok_or(Error::new(EBADF))?;

                    // Partition size in bytes.
                    let len = part.size * BLK_SIZE;

                    log::debug!("part: part_len={len:?}");

                    len
                }

                Handle::Disk => self.cfg.capacity() * u64::from(self.cfg.block_size()),
            },
        ))
    }

    fn fpath(&mut self, _id: usize, _buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fstat(&mut self, id: usize, _stat: &mut syscall::Stat) -> syscall::Result<Option<usize>> {
        match self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List { .. } => Ok(Some(0)),
            Handle::Disk { .. } | Handle::Partition { .. } => todo!(),
        }
    }

    fn dup(&mut self, _old_id: usize, _buf: &[u8]) -> Result<Option<usize>> {
        todo!()
    }

    fn close(&mut self, id: usize) -> syscall::Result<Option<usize>> {
        self.handles
            .remove(&id)
            .ok_or(Error::new(EBADF))
            .and(Ok(Some(0)))
    }
}
