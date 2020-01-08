pub fn read<E>(offset: u64, blksize: u32, mut buf: &mut [u8], block_bytes: &mut [u8], mut read: impl FnMut(u64, &mut [u8]) -> Result<(), E>) -> Result<usize, E> {
    // TODO: Yield sometimes, perhaps after a few blocks or something.
    use std::ops::{Add, Div, Rem};

    fn div_round_up<T>(a: T, b: T) -> T
    where
        T: Add<Output = T> + Div<Output = T> + Rem<Output = T> + PartialEq + From<u8> + Copy
    {
        if a % b != T::from(0u8) {
            a / b + T::from(1u8)
        } else {
            a / b
        }
    }

    let orig_buf_len = buf.len();

    let start_block = offset / u64::from(blksize);
    let end_block = div_round_up(offset + buf.len() as u64, u64::from(blksize)); // The first block not in the range

    let offset_from_start_block: u64 = offset % u64::from(blksize);
    let offset_to_end_block: u64 = u64::from(blksize) - (offset + buf.len() as u64) % u64::from(blksize);

    let first_whole_block = start_block + if offset_from_start_block > 0 { 1 } else { 0 };
    let last_whole_block = end_block - if offset_to_end_block > 0 { 1 } else { 0 } - 1;

    let whole_blocks_to_read = last_whole_block - first_whole_block + 1;

    for block in start_block..=end_block {
        // TODO: Async/await? I mean, shouldn't AHCI be async?

        read(block, block_bytes)?;

        let (bytes_to_read, src_buf): (u64, &[u8]) = if block == start_block {
            (u64::from(blksize) - offset_from_start_block, &block_bytes[offset_from_start_block as usize..])
        } else if block == end_block {
            (u64::from(blksize) - offset_to_end_block, &block_bytes[..offset_to_end_block as usize])
        } else {
            (blksize.into(), &block_bytes[..])
        };
        let bytes_to_read = std::cmp::min(bytes_to_read as usize, buf.len());
        buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
        buf = &mut buf[bytes_to_read..];
    }

    Ok(std::cmp::min(orig_buf_len, whole_blocks_to_read as usize * blksize as usize + offset_from_start_block as usize + offset_to_end_block as usize))
}
