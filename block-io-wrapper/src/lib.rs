use std::cmp::min;
use std::io::{Error, ErrorKind};

/// Split the read operation into a series of block reads.
/// `read_fn` will be called with a block number to be read, and a buffer to be filled.
/// The buffer must be large enough to hold `blksize` of data.
/// `read_fn` must return a full block of data.
/// Result will be the number of bytes read.
pub fn read(
    offset: u64,
    blksize: u32,
    mut buf: &mut [u8],
    block_bytes: &mut [u8],
    mut read_fn: impl FnMut(u64, &mut [u8]) -> Result<(), Error>,
) -> Result<usize, Error> {
    // TODO: Yield sometimes, perhaps after a few blocks or something.

    if buf.len() == 0 {
        return Ok(0);
    }
    let (_, would_overflow) = offset.overflowing_add(buf.len() as u64);
    let to_copy = if would_overflow {
        usize::try_from(u64::MAX - offset).map_err(|_| Error::new(ErrorKind::Unsupported, "offset + len exceeds usize::MAX"))?
    } else {
        buf.len()
    };
    let mut curr_buf = &mut buf[..to_copy];
    let mut curr_offset = offset;
    let blk_size = usize::try_from(blksize).expect("blksize larger than usize");
    let mut total_read = 0;

    while curr_buf.len() > 0 {
        // TODO: Async/await? I mean, shouldn't AHCI be async?

        let blk_offset =
            usize::try_from(curr_offset % u64::from(blksize)).expect("usize smaller than blksize");
        let to_copy = min(curr_buf.len(), blk_size - blk_offset);
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
