use std::cmp;
use std::collections::BTreeMap;
use std::io::Read;
use std::io::Result as IoResult;
use std::io::Seek;

use std::fmt::Write;
use std::sync::Arc;

use partitionlib::LogicalBlockSize;
use partitionlib::PartitionTable;
use syscall::*;
use virtiod::transport::Queue;
use virtiod::Buffer;
use virtiod::ChainBuilder;
use virtiod::DescriptorFlags;

use crate::BlockDeviceConfig;
use crate::BlockRequestTy;
use crate::BlockVirtRequest;

const BLK_SIZE: u64 = 512;

trait BlkExtension {
    /// XXX: Reads only one block despite the size of the output buffer. Use [`BlkExtension::read`] instead.
    fn read_block(&self, block: u64, block_bytes: &mut [u8]) -> usize;
    fn read(&self, block: u64, block_bytes: &mut [u8]) -> usize {
        let sectors = block_bytes.len() / BLK_SIZE as usize;

        (0..sectors)
            .map(|i| self.read_block(block + i as u64, &mut block_bytes[i * BLK_SIZE as usize..]))
            .sum()
    }
}

impl BlkExtension for Queue<'_> {
    fn read_block(&self, block: u64, block_bytes: &mut [u8]) -> usize {
        let req = syscall::Dma::new(BlockVirtRequest {
            ty: BlockRequestTy::In,
            reserved: 0,
            sector: block,
        })
        .unwrap();

        let result = syscall::Dma::new([0u8; 512]).unwrap();
        let status = syscall::Dma::new(u8::MAX).unwrap();

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&req).flags(DescriptorFlags::NEXT))
            .chain(Buffer::new(&result).flags(DescriptorFlags::WRITE_ONLY | DescriptorFlags::NEXT))
            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.send(chain);

        // FIXME: interrupts are for a reason
        while *status != 0 {
            core::hint::spin_loop();
        }

        let size = core::cmp::min(block_bytes.len(), result.len());
        block_bytes[..size].copy_from_slice(&result.as_slice()[..size]);
        std::thread::yield_now();

        size
    }
}

pub enum Handle {
    Partition {
        /// Partition Number
        number: u32,
        /// Offset in bytes
        offset: usize,
    },

    List {
        entries: Vec<u8>,
        offset: usize,
    },

    Disk {
        offset: usize,
    },
}

pub struct DiskScheme<'a> {
    name: String,
    queue: Arc<Queue<'a>>,
    next_id: usize,
    cfg: &'a mut BlockDeviceConfig,
    handles: BTreeMap<usize, Handle>,
    part_table: Option<PartitionTable>,
}

impl<'a> DiskScheme<'a> {
    pub fn new(name: String, queue: Arc<Queue<'a>>, cfg: &'a mut BlockDeviceConfig) -> Self {
        let mut this = Self {
            name,
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
                let read_block = |block: u64, block_bytes: &mut [u8]| -> Result<(), ()> {
                    let req = syscall::Dma::new(BlockVirtRequest {
                        ty: BlockRequestTy::In,
                        reserved: 0,
                        sector: block,
                    })
                    .unwrap();

                    let result = syscall::Dma::new([0u8; 512]).unwrap();
                    let status = syscall::Dma::new(u8::MAX).unwrap();

                    let chain = ChainBuilder::new()
                        .chain(Buffer::new(&req).flags(DescriptorFlags::NEXT))
                        .chain(
                            Buffer::new(&result)
                                .flags(DescriptorFlags::WRITE_ONLY | DescriptorFlags::NEXT),
                        )
                        .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
                        .build();

                    self.scheme.queue.send(chain);

                    // FIXME: interrupts are for a reason
                    while *status != 0 {
                        core::hint::spin_loop();
                    }

                    let size = core::cmp::min(block_bytes.len(), result.len());
                    block_bytes[..size].copy_from_slice(&result.as_slice()[..size]);

                    std::thread::yield_now();

                    Ok(())
                };

                let bytes_read =
                    block_io_wrapper::read(self.offset, 512, buf, self.block_bytes, read_block)
                        .unwrap();
                self.offset += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        impl<'a, 'b> Seek for VirtioShim<'a, 'b> {
            fn seek(&mut self, from: std::io::SeekFrom) -> IoResult<u64> {
                let size_u = self.scheme.cfg.capacity.get() * self.scheme.cfg.blk_size.get() as u64;
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
            .unwrap()
            .expect("virtiod: no partitions found");

        this.part_table = Some(part_table);
        this
    }
}

impl<'a> SchemeBlockMut for DiskScheme<'a> {
    fn open(
        &mut self,
        path: &str,
        flags: usize,
        uid: u32,
        gid: u32,
    ) -> syscall::Result<Option<usize>> {
        log::info!("virtioo: open: {}", path);

        let path_str = path.trim_matches('/');
        if path_str.is_empty() {
            if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                let mut list = String::new();
                // FIXME: The zero is the disk identifier (look in the nvmed scheme, it set's this
                //            to the namespace id).
                write!(list, "{}\n", 0).unwrap();

                let part_table = self.part_table.as_ref().unwrap();

                for part_num in 0..part_table.partitions.len() {
                    write!(list, "{}p{}\n", 0, part_num).unwrap();
                }

                let id = self.next_id;
                self.next_id += 1;
                self.handles.insert(
                    id,
                    Handle::List {
                        entries: list.into_bytes(),
                        offset: 0,
                    },
                );

                Ok(Some(id))
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
            self.handles.insert(
                id,
                Handle::Partition {
                    number: part_num,
                    offset: 0,
                },
            );

            Ok(Some(id))
        } else {
            let nsid = path_str.parse::<u32>().unwrap();
            assert_eq!(nsid, 0);

            let id = self.next_id;
            self.next_id += 1;
            self.handles.insert(id, Handle::Disk { offset: 0 });
            Ok(Some(id))
        }
    }

    fn chmod(
        &mut self,
        path: &str,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn rmdir(&mut self, path: &str, uid: u32, gid: u32) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn unlink(&mut self, path: &str, uid: u32, gid: u32) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn dup(&mut self, old_id: usize, buf: &[u8]) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List {
                ref mut entries,
                ref mut offset,
            } => {
                let count = (&entries[*offset..]).read(buf).unwrap();
                *offset += count;
                Ok(Some(count))
            }

            Handle::Partition {
                number,
                ref mut offset,
            } => {
                let part_table = self.part_table.as_ref().unwrap();
                let part = part_table
                    .partitions
                    .get(number as usize)
                    .ok_or(Error::new(EBADF))?;

                // Get the offset in sectors.
                let rel_block = (*offset as u64) / BLK_SIZE;
                // if rel_block >= part.size {
                //     return Err(Error::new(EOVERFLOW));
                // }

                let abs_block = part.start_lba + rel_block;

                let count = self.queue.read(abs_block, buf);
                *offset += count;
                Ok(Some(count))
            }

            Handle::Disk { ref mut offset } => {
                let block_size = self.cfg.block_size();

                let count = self.queue.read((*offset as u64) / 512, buf);
                *offset += count;
                Ok(Some(count))
            }
        }
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<Option<usize>> {
        // lets assume this worked
        Ok(Some(buf.len()))
    }

    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> syscall::Result<Option<isize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List {
                ref entries,
                ref mut offset,
            } => {
                let len = entries.len() as isize;
                log::debug!("list: whence={whence:?} pos={pos:?} part_len={len:?}");

                *offset = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len, *offset as isize + pos)),
                    SEEK_END => cmp::max(0, cmp::min(len, len + pos)),
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(Some(*offset as isize))
            }

            Handle::Partition {
                number,
                ref mut offset,
            } => {
                let part_table = self.part_table.as_ref().unwrap();
                let part = part_table
                    .partitions
                    .get(number as usize)
                    .ok_or(Error::new(EBADF))?;

                // Partition size in bytes.
                let len = (part.size * BLK_SIZE) as isize;

                log::debug!("part: whence={whence:?} pos={pos:?} part_len={len:?}");

                *offset = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len, *offset as isize + pos)),
                    SEEK_END => cmp::max(0, cmp::min(len, len + pos)),
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(Some(*offset as isize))
            }

            Handle::Disk { ref mut offset } => {
                let len = (self.cfg.capacity() * self.cfg.block_size() as u64) as isize;

                *offset = match whence {
                    SEEK_SET => cmp::min(len, pos),
                    SEEK_CUR => cmp::max(0, cmp::min(len, *offset as isize + pos)),
                    SEEK_END => cmp::max(0, cmp::min(len, len + pos)),
                    _ => return Err(Error::new(EINVAL)),
                } as usize;

                Ok(Some(*offset as isize))
            }
        }
    }

    fn fchmod(&mut self, id: usize, mode: u16) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fchown(&mut self, id: usize, uid: u32, gid: u32) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fcntl(&mut self, id: usize, cmd: usize, arg: usize) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fevent(
        &mut self,
        id: usize,
        flags: syscall::EventFlags,
    ) -> syscall::Result<Option<syscall::EventFlags>> {
        todo!()
    }

    fn fmap_old(&mut self, id: usize, map: &syscall::OldMap) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fmap(&mut self, id: usize, map: &syscall::Map) -> syscall::Result<Option<usize>> {
        if map.flags.contains(syscall::MapFlags::MAP_FIXED) {
            return Err(syscall::Error::new(syscall::EINVAL));
        }
        self.fmap_old(
            id,
            &syscall::OldMap {
                offset: map.offset,
                size: map.size,
                flags: map.flags,
            },
        )
    }

    fn funmap_old(&mut self, address: usize) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn funmap(&mut self, address: usize, length: usize) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn frename(
        &mut self,
        id: usize,
        path: &str,
        uid: u32,
        gid: u32,
    ) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fstat(&mut self, id: usize, stat: &mut syscall::Stat) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fstatvfs(
        &mut self,
        id: usize,
        stat: &mut syscall::StatVfs,
    ) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn ftruncate(&mut self, id: usize, len: usize) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn futimens(
        &mut self,
        id: usize,
        times: &[syscall::TimeSpec],
    ) -> syscall::Result<Option<usize>> {
        todo!()
    }

    fn close(&mut self, id: usize) -> syscall::Result<Option<usize>> {
        self.handles
            .remove(&id)
            .ok_or(Error::new(EBADF))
            .and(Ok(Some(0)))
    }
}
