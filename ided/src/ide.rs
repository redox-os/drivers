use std::{
    sync::{Arc, Mutex},
    thread,
};
use syscall::{
    error::{Error, Result, EIO},
    io::{Io, Pio, ReadOnly, WriteOnly},
};

use crate::ata::AtaCommand;

pub struct Channel {
    pub data8: Pio<u8>,
    pub data32: Pio<u32>,
    pub error: ReadOnly<Pio<u8>>,
    pub features: WriteOnly<Pio<u8>>,
    pub sector_count: Pio<u8>,
    pub lba_0: Pio<u8>,
    pub lba_1: Pio<u8>,
    pub lba_2: Pio<u8>,
    pub device_select: Pio<u8>,
    pub status: ReadOnly<Pio<u8>>,
    pub command: WriteOnly<Pio<u8>>,
    pub alt_status: ReadOnly<Pio<u8>>,
    pub control: WriteOnly<Pio<u8>>,
}

impl Channel {
    pub fn new(base: u16, control_base: u16) -> Self {
        Self {
            data8: Pio::new(base + 0),
            data32: Pio::new(base + 0),
            error: ReadOnly::new(Pio::new(base + 1)),
            features: WriteOnly::new(Pio::new(base + 1)),
            sector_count: Pio::new(base + 2),
            lba_0: Pio::new(base + 3),
            lba_1: Pio::new(base + 4),
            lba_2: Pio::new(base + 5),
            device_select: Pio::new(base + 6),
            status: ReadOnly::new(Pio::new(base + 7)),
            command: WriteOnly::new(Pio::new(base + 7)),
            alt_status: ReadOnly::new(Pio::new(control_base + 2)),
            control: WriteOnly::new(Pio::new(control_base + 2)),
        }
    }

    pub fn primary_compat() -> Self {
        Self::new(0x1F0, 0x3F6)
    }

    pub fn secondary_compat() -> Self {
        Self::new(0x170, 0x376)
    }

    fn polling(&mut self, check: bool) -> Result<()> {
        /*
        #define ATA_SR_BSY     0x80    // Busy
        #define ATA_SR_DRDY    0x40    // Drive ready
        #define ATA_SR_DF      0x20    // Drive write fault
        #define ATA_SR_DSC     0x10    // Drive seek complete
        #define ATA_SR_DRQ     0x08    // Data request ready
        #define ATA_SR_CORR    0x04    // Corrected data
        #define ATA_SR_IDX     0x02    // Index
        #define ATA_SR_ERR     0x01    // Error
        */

        for _ in 0..4 {
            // Doing this 4 times creates a 400ns delay
            self.alt_status.read();
        }

        while self.status.readf(0x80) {
            thread::yield_now();
        }

        if check {
            let status = self.status.read();

            if status & 0x01 != 0 {
                log::error!("IDE error");
                return Err(Error::new(EIO));
            }

            if status & 0x20 != 0 {
                log::error!("IDE device write fault");
                return Err(Error::new(EIO));
            }

            if status & 0x08 == 0 {
                log::error!("IDE data not ready");
                return Err(Error::new(EIO));
            }
        }

        Ok(())
    }
}

pub trait Disk {
    fn id(&self) -> usize;
    fn size(&mut self) -> u64;
    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<Option<usize>>;
    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<Option<usize>>;
    fn block_length(&mut self) -> Result<u32>;
}

pub struct AtaDisk {
    pub chan: Arc<Mutex<Channel>>,
    pub chan_i: usize,
    pub dev: u8,
    pub size: u64,
}

impl Disk for AtaDisk {
    fn id(&self) -> usize {
        self.chan_i << 1 | self.dev as usize
    }

    fn size(&mut self) -> u64 {
        self.size
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<Option<usize>> {
        //TODO: support other LBA modes
        assert!(block < 0x1_0000_0000_0000);

        let sectors = buffer.len() / 512;
        assert!(sectors < 0x1_0000);

        let mut chan = self.chan.lock().unwrap();

        // Select drive
        chan.device_select.write(0xE0 | (self.dev << 4));

        // Set high sector count and LBA
        //TODO: only if LBA mode is 48-bit
        chan.control.writef(0x80, true);
        chan.sector_count.write((sectors >> 8) as u8);
        chan.lba_0.write((block >> 24) as u8);
        chan.lba_1.write((block >> 32) as u8);
        chan.lba_2.write((block >> 40) as u8);
        chan.control.writef(0x80, false);

        // Set low sector count and LBA
        chan.sector_count.write(sectors as u8);
        chan.lba_0.write(block as u8);
        chan.lba_1.write((block >> 8) as u8);
        chan.lba_2.write((block >> 16) as u8);

        // Send command
        //TODO: use DMA
        chan.command.write(AtaCommand::ReadPioExt as u8);

        // Read data
        for sector in 0..sectors {
            chan.polling(true)?;

            for i in 0..128 {
                let data = chan.data32.read();
                buffer[sector * 512 + i * 4 + 0] = (data >> 0) as u8;
                buffer[sector * 512 + i * 4 + 1] = (data >> 8) as u8;
                buffer[sector * 512 + i * 4 + 2] = (data >> 16) as u8;
                buffer[sector * 512 + i * 4 + 3] = (data >> 24) as u8;
            }
        }

        Ok(Some(sectors * 512))
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<Option<usize>> {
        //TODO: support other LBA modes
        assert!(block < 0x1_0000_0000_0000);

        let sectors = buffer.len() / 512;
        assert!(sectors < 0x1_0000);

        let mut chan = self.chan.lock().unwrap();

        // Select drive
        chan.device_select.write(0xE0 | (self.dev << 4));

        // Set high sector count and LBA
        //TODO: only if LBA mode is 48-bit
        chan.control.writef(0x80, true);
        chan.sector_count.write((sectors >> 8) as u8);
        chan.lba_0.write((block >> 24) as u8);
        chan.lba_1.write((block >> 32) as u8);
        chan.lba_2.write((block >> 40) as u8);
        chan.control.writef(0x80, false);

        // Set low sector count and LBA
        chan.sector_count.write(sectors as u8);
        chan.lba_0.write(block as u8);
        chan.lba_1.write((block >> 8) as u8);
        chan.lba_2.write((block >> 16) as u8);

        // Send command
        //TODO: use DMA
        chan.command.write(AtaCommand::WritePioExt as u8);

        // Write data
        for sector in 0..sectors {
            chan.polling(false)?;

            for i in 0..128 {
                chan.data32.write(
                    ((buffer[sector * 512 + i * 4 + 0] as u32) << 0) |
                    ((buffer[sector * 512 + i * 4 + 1] as u32) << 8) |
                    ((buffer[sector * 512 + i * 4 + 2] as u32) << 16) |
                    ((buffer[sector * 512 + i * 4 + 3] as u32) << 24)
                );
            }
        }

        chan.command.write(AtaCommand::CacheFlushExt as u8);
        chan.polling(false)?;

        Ok(Some(sectors * 512))
    }

    fn block_length(&mut self) -> Result<u32> {
        Ok(512)
    }
}
