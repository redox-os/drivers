#![allow(dead_code)]

use byteorder::{ByteOrder, BigEndian};

use syscall::io::Dma;
use syscall::error::{Result, ENOSYS, Error};

use super::hba::{HbaPort, HbaCmdTable, HbaCmdHeader};
use super::Disk;

const SCSI_READ_CAPACITY: u8 = 0x25;

pub struct DiskATAPI {
    id: usize,
    port: &'static mut HbaPort,
    size: u64,
    clb: Dma<[HbaCmdHeader; 32]>,
    ctbas: [Dma<HbaCmdTable>; 32],
    _fb: Dma<[u8; 256]>,
    buf: Dma<[u8; 256 * 512]>
}

impl DiskATAPI {
    pub fn new(id: usize, port: &'static mut HbaPort) -> Result<Self> {
        let mut clb = Dma::zeroed()?;
        let mut ctbas = [
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
        ];
        let mut fb = Dma::zeroed()?;
        let buf = Dma::zeroed()?;

        port.init(&mut clb, &mut ctbas, &mut fb);

        let size = unsafe { port.identify_packet(&mut clb, &mut ctbas).unwrap_or(0) };

        Ok(DiskATAPI {
            id: id,
            port: port,
            size: size,
            clb: clb,
            ctbas: ctbas,
            _fb: fb,
            buf: buf
        })
    }

    fn read_capacity(&mut self) -> Result<(u32, u32)> {
        // TODO: only query when needed (disk changed)

        let mut cmd = [0; 16];
        cmd[0] = SCSI_READ_CAPACITY;
        self.port.packet(&cmd, 8, &mut self.clb, &mut self.ctbas, &mut self.buf)?;

        let blk_count = BigEndian::read_u32(&self.buf[0..4]);
        let blk_size = BigEndian::read_u32(&self.buf[4..8]);

        Ok((blk_count, blk_size))
    }
}

impl Disk for DiskATAPI {
    fn id(&self) -> usize {
        self.id
    }

    fn size(&mut self) -> u64 {
        match self.read_capacity() {
            Ok((blk_count, blk_size)) => (blk_count as u64) * (blk_size as u64),
            Err(_) => 0 // XXX
        }
    }

    fn read(&mut self, _block: u64, _buffer: &mut [u8]) -> Result<usize> {
        Err(Error::new(ENOSYS))
    }

    fn write(&mut self, _block: u64, _buffer: &[u8]) -> Result<usize> {
        Err(Error::new(ENOSYS)) // TODO: Implement writting
    }
    
}
