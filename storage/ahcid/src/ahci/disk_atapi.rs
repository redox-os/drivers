#![allow(dead_code)]

use std::convert::TryInto;
use std::ptr;

use byteorder::{BigEndian, ByteOrder};

use syscall::error::{Error, Result, EBADF};

use common::dma::Dma;

use super::hba::{HbaCmdHeader, HbaCmdTable, HbaPort};
use super::Disk;

const SCSI_READ_CAPACITY: u8 = 0x25;
const SCSI_READ10: u8 = 0x28;

pub struct DiskATAPI {
    id: usize,
    port: &'static mut HbaPort,
    size: u64,
    clb: Dma<[HbaCmdHeader; 32]>,
    ctbas: [Dma<HbaCmdTable>; 32],
    _fb: Dma<[u8; 256]>,
    // Just using the same buffer size as DiskATA
    // Although the sector size is different (and varies)
    buf: Dma<[u8; 256 * 512]>,
    blk_count: u32,
    blk_size: u32,
}

impl DiskATAPI {
    pub fn new(id: usize, port: &'static mut HbaPort) -> Result<Self> {
        let mut clb = unsafe { Dma::zeroed()?.assume_init() };

        let mut ctbas: [_; 32] = (0..32)
            .map(|_| Ok(unsafe { Dma::zeroed()?.assume_init() }))
            .collect::<Result<Vec<_>>>()?
            .try_into()
            .unwrap_or_else(|_| unreachable!());

        let mut fb = unsafe { Dma::zeroed()?.assume_init() };
        let mut buf = unsafe { Dma::zeroed()?.assume_init() };

        port.init(&mut clb, &mut ctbas, &mut fb)?;

        let size = unsafe { port.identify_packet(&mut clb, &mut ctbas).unwrap_or(0) };

        let mut cmd = [0; 16];
        cmd[0] = SCSI_READ_CAPACITY;
        port.atapi_dma(&cmd, 8, &mut clb, &mut ctbas, &mut buf)?;

        // Instead of a count, contains number of last LBA, so add 1
        let blk_count = BigEndian::read_u32(&buf[0..4]) + 1;
        let blk_size = BigEndian::read_u32(&buf[4..8]);

        Ok(DiskATAPI {
            id,
            port,
            size,
            clb,
            ctbas,
            _fb: fb,
            buf,
            blk_count,
            blk_size,
        })
    }
}

impl Disk for DiskATAPI {
    fn block_size(&self) -> u32 {
        self.blk_size
    }

    fn size(&self) -> u64 {
        u64::from(self.blk_count) * u64::from(self.blk_size)
    }

    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize> {
        // TODO: Handle audio CDs, which use special READ CD command

        let blk_len = self.blk_size;
        let sectors = buffer.len() as u32 / blk_len;

        fn read10_cmd(block: u32, count: u16) -> [u8; 16] {
            let mut cmd = [0; 16];
            cmd[0] = SCSI_READ10;
            BigEndian::write_u32(&mut cmd[2..6], block as u32);
            BigEndian::write_u16(&mut cmd[7..9], count as u16);
            cmd
        }

        let mut sector = 0;
        let buf_len = (256 * 512) / blk_len;
        let buf_size = buf_len * blk_len;
        while sectors - sector >= buf_len {
            let cmd = read10_cmd(block as u32 + sector, buf_len as u16);
            self.port.atapi_dma(
                &cmd,
                buf_size,
                &mut self.clb,
                &mut self.ctbas,
                &mut self.buf,
            )?;

            unsafe {
                ptr::copy(
                    self.buf.as_ptr(),
                    buffer
                        .as_mut_ptr()
                        .offset(sector as isize * blk_len as isize),
                    buf_size as usize,
                );
            }

            sector += blk_len;
        }
        if sector < sectors {
            let cmd = read10_cmd(block as u32 + sector, (sectors - sector) as u16);
            self.port.atapi_dma(
                &cmd,
                buf_size,
                &mut self.clb,
                &mut self.ctbas,
                &mut self.buf,
            )?;

            unsafe {
                ptr::copy(
                    self.buf.as_ptr(),
                    buffer
                        .as_mut_ptr()
                        .offset(sector as isize * blk_len as isize),
                    ((sectors - sector) * blk_len) as usize,
                );
            }

            sector += sectors - sector;
        }

        Ok((sector * blk_len) as usize)
    }

    async fn write(&mut self, _block: u64, _buffer: &[u8]) -> Result<usize> {
        Err(Error::new(EBADF)) // TODO: Implement writing
    }
}
