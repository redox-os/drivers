use std::cmp;
use std::io::Error;
use std::io::{self, Read, Seek, SeekFrom};

use partitionlib::{LogicalBlockSize, PartitionTable};

/// Split the read operation into a series of block reads.
/// `read_fn` will be called with a block number to be read, and a buffer to be filled.
/// The buffer must be large enough to hold `blksize` of data.
/// `read_fn` must return a full block of data.
/// Result will be the number of bytes read.
// FIXME make private once nvmed uses the DiskWrapper defined in this crate
pub fn block_read(
    offset: u64,
    blksize: u32,
    buf: &mut [u8],
    block_bytes: &mut [u8],
    mut read_fn: impl FnMut(u64, &mut [u8]) -> Result<(), Error>,
) -> Result<usize, Error> {
    // TODO: Yield sometimes, perhaps after a few blocks or something.

    if buf.len() == 0 {
        return Ok(0);
    }
    let to_copy = usize::try_from(
        offset.saturating_add(u64::try_from(buf.len()).expect("buf.len() larger than u64"))
            - offset,
    )
    .expect("bytes to copy larger than usize");
    let mut curr_buf = &mut buf[..to_copy];
    let mut curr_offset = offset;
    let blk_size = usize::try_from(blksize).expect("blksize larger than usize");
    let mut total_read = 0;

    while curr_buf.len() > 0 {
        // TODO: Async/await? I mean, shouldn't AHCI be async?

        let blk_offset =
            usize::try_from(curr_offset % u64::from(blksize)).expect("usize smaller than blksize");
        let to_copy = cmp::min(curr_buf.len(), blk_size - blk_offset);
        assert!(blk_offset + to_copy <= blk_size);

        read_fn(curr_offset / u64::from(blksize), block_bytes)?;

        let src_buf = &block_bytes[blk_offset..];

        curr_buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);
        curr_buf = &mut curr_buf[to_copy..];
        curr_offset += u64::try_from(to_copy).expect("bytes to copy larger than u64");
        total_read += to_copy;
    }
    Ok(total_read)
}

pub trait Disk {
    fn id(&self) -> usize;
    fn block_length(&mut self) -> syscall::error::Result<u32>;
    fn size(&mut self) -> u64;

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<Option<usize>>;
    fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<Option<usize>>;
}

pub struct DiskWrapper {
    pub disk: Box<dyn Disk>,
    pub pt: Option<PartitionTable>,
}

impl DiskWrapper {
    fn pt(disk: &mut dyn Disk) -> Option<PartitionTable> {
        let bs = match disk.block_length() {
            Ok(512) => LogicalBlockSize::Lb512,
            _ => return None,
        };
        struct Device<'a, 'b> {
            disk: &'a mut dyn Disk,
            offset: u64,
            block_bytes: &'b mut [u8],
        }

        impl<'a, 'b> Seek for Device<'a, 'b> {
            fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
                let size = i64::try_from(self.disk.size()).or(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Disk larger than 2^63 - 1 bytes",
                )))?;

                self.offset = match from {
                    SeekFrom::Start(new_pos) => cmp::min(self.disk.size(), new_pos),
                    SeekFrom::Current(new_pos) => {
                        cmp::max(0, cmp::min(size, self.offset as i64 + new_pos)) as u64
                    }
                    SeekFrom::End(new_pos) => cmp::max(0, cmp::min(size + new_pos, size)) as u64,
                };

                Ok(self.offset)
            }
        }
        // TODO: Perhaps this impl should be used in the rest of the scheme.
        impl<'a, 'b> Read for Device<'a, 'b> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let blksize = self
                    .disk
                    .block_length()
                    .map_err(|err| io::Error::from_raw_os_error(err.errno))?;
                let size_in_blocks = self.disk.size() / u64::from(blksize);

                let disk = &mut self.disk;

                let read_block = |block: u64, block_bytes: &mut [u8]| {
                    if block >= size_in_blocks {
                        return Err(io::Error::from_raw_os_error(syscall::EOVERFLOW));
                    }
                    loop {
                        match disk.read(block, block_bytes) {
                            Ok(Some(bytes)) => {
                                assert_eq!(bytes, block_bytes.len());
                                assert_eq!(bytes, blksize as usize);
                                return Ok(());
                            }
                            Ok(None) => {
                                std::thread::yield_now();
                                continue;
                            }
                            Err(err) => return Err(io::Error::from_raw_os_error(err.errno)),
                        }
                    }
                };
                let bytes_read =
                    block_read(self.offset, blksize, buf, self.block_bytes, read_block)?;

                self.offset += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        let mut block_bytes = [0u8; 4096];

        partitionlib::get_partitions(
            &mut Device {
                disk,
                offset: 0,
                block_bytes: &mut block_bytes[..bs.into()],
            },
            bs,
        )
        .ok()
        .flatten()
    }

    pub fn new(mut disk: Box<dyn Disk>) -> Self {
        Self {
            pt: Self::pt(&mut *disk),
            disk,
        }
    }
}

impl std::ops::Deref for DiskWrapper {
    type Target = dyn Disk;

    fn deref(&self) -> &Self::Target {
        &*self.disk
    }
}
impl std::ops::DerefMut for DiskWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.disk
    }
}
