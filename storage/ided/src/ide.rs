use std::{
    convert::TryInto,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use driver_block::Disk;
use syscall::error::{Error, Result, EIO};

use common::dma::Dma;
use common::io::{Io, Pio, ReadOnly, WriteOnly};

static TIMEOUT: Duration = Duration::new(1, 0);

#[repr(u8)]
pub enum AtaCommand {
    ReadPio = 0x20,
    ReadPioExt = 0x24,
    ReadDma = 0xC8,
    ReadDmaExt = 0x25,
    WritePio = 0x30,
    WritePioExt = 0x34,
    WriteDma = 0xCA,
    WriteDmaExt = 0x35,
    CacheFlush = 0xE7,
    CacheFlushExt = 0xEA,
    Packet = 0xA0,
    IdentifyPacket = 0xA1,
    Identify = 0xEC,
}

#[repr(C, packed)]
struct PrdtEntry {
    phys: u32,
    size: u16,
    flags: u16,
}

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
    pub busmaster_command: Pio<u8>,
    pub busmaster_status: Pio<u8>,
    pub busmaster_prdt: Pio<u32>,
    prdt: Dma<[PrdtEntry; 128]>,
    buf: Dma<[u8; 128 * 512]>,
}

impl Channel {
    pub fn new(base: u16, control_base: u16, busmaster_base: u16) -> Result<Self> {
        Ok(Self {
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
            alt_status: ReadOnly::new(Pio::new(control_base)),
            control: WriteOnly::new(Pio::new(control_base)),
            busmaster_command: Pio::new(busmaster_base),
            busmaster_status: Pio::new(busmaster_base + 2),
            busmaster_prdt: Pio::new(busmaster_base + 4),
            prdt: unsafe {
                Dma::zeroed(
                    //TODO: PhysBox::new_in_32bit_space(4096)?
                )?
                .assume_init()
            },
            buf: unsafe {
                Dma::zeroed(
                    //TODO: PhysBox::new_in_32bit_space(16 * 4096)?
                )?
                .assume_init()
            },
        })
    }

    pub fn primary_compat(busmaster_base: u16) -> Result<Self> {
        Self::new(0x1F0, 0x3F6, busmaster_base)
    }

    pub fn secondary_compat(busmaster_base: u16) -> Result<Self> {
        Self::new(0x170, 0x376, busmaster_base)
    }

    fn check_status(&mut self) -> Result<u8> {
        let status = self.status.read();

        if status & 0x01 != 0 {
            log::error!("IDE error: {:#x}", self.error.read());
            return Err(Error::new(EIO));
        }

        if status & 0x20 != 0 {
            log::error!("IDE device write fault");
            return Err(Error::new(EIO));
        }

        Ok(status)
    }

    fn polling(&mut self, read: bool, line: u32) -> Result<()> {
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

        let start = Instant::now();
        loop {
            let status = self.check_status()?;
            if status & 0x80 == 0 {
                if read && status & 0x08 == 0 {
                    log::error!("IDE read data not ready");
                    return Err(Error::new(EIO));
                }
                break;
            }
            if start.elapsed() >= TIMEOUT {
                log::error!(
                    "line {} polling {} timeout with status 0x{:02X}",
                    line,
                    if read { "read" } else { "write" },
                    status
                );
                return Err(Error::new(EIO));
            }
            thread::yield_now();
        }

        Ok(())
    }
}

pub struct AtaDisk {
    pub chan: Arc<Mutex<Channel>>,
    pub chan_i: usize,
    pub dev: u8,
    pub size: u64,
    pub dma: bool,
    pub lba_48: bool,
}

impl Disk for AtaDisk {
    fn block_size(&self) -> u32 {
        512
    }

    fn size(&self) -> u64 {
        self.size
    }

    // NOTE: not async
    async fn read(&mut self, start_block: u64, buffer: &mut [u8]) -> Result<usize> {
        let mut count = 0;
        for chunk in buffer.chunks_mut(65536) {
            let block = start_block + (count as u64) / 512;

            //TODO: support other LBA modes
            assert!(block < 0x1_0000_0000_0000);

            let sectors = (chunk.len() + 511) / 512;
            assert!(sectors <= 128);

            log::trace!(
                "IDE read chan {} dev {} block {:#x} count {:#x}",
                self.chan_i,
                self.dev,
                block,
                sectors
            );

            let mut chan = self.chan.lock().unwrap();

            if self.dma {
                // Stop bus master
                chan.busmaster_command.writef(1, false);
                // Make PRDT EOT match chunk size
                for i in 0..sectors {
                    chan.prdt[i] = PrdtEntry {
                        phys: (chan.buf.physical() + i * 512).try_into().unwrap(),
                        size: 512,
                        flags: if i + 1 == sectors {
                            1 << 15 // End of table
                        } else {
                            0
                        },
                    };
                }
                // Set PRDT
                let prdt = chan.prdt.physical();
                chan.busmaster_prdt.write(prdt.try_into().unwrap());
                // Set to read
                chan.busmaster_command.writef(1 << 3, true);
                // Clear interrupt and error bits
                chan.busmaster_status.write(0b110);
            }

            // Select drive
            //TODO: upper part of LBA 28
            chan.device_select.write(0xE0 | (self.dev << 4));

            if self.lba_48 {
                // Set high sector count and LBA
                chan.control.writef(0x80, true);
                chan.sector_count.write((sectors >> 8) as u8);
                chan.lba_0.write((block >> 24) as u8);
                chan.lba_1.write((block >> 32) as u8);
                chan.lba_2.write((block >> 40) as u8);
                chan.control.writef(0x80, false);
            }

            // Set low sector count and LBA
            chan.sector_count.write(sectors as u8);
            chan.lba_0.write(block as u8);
            chan.lba_1.write((block >> 8) as u8);
            chan.lba_2.write((block >> 16) as u8);

            // Send command
            chan.command.write(if self.dma {
                if self.lba_48 {
                    AtaCommand::ReadDmaExt as u8
                } else {
                    AtaCommand::ReadDma as u8
                }
            } else {
                if self.lba_48 {
                    AtaCommand::ReadPioExt as u8
                } else {
                    AtaCommand::ReadPio as u8
                }
            });

            // Read data
            if self.dma {
                // Start bus master
                chan.busmaster_command.writef(1, true);

                // Wait for transaction to finish
                chan.polling(false, line!())?;

                // Wait for bus master to finish
                let start = Instant::now();
                let error = loop {
                    let status = chan.busmaster_status.read();
                    if status & 1 << 1 != 0 {
                        // Break with error status
                        break true;
                    }
                    if status & 1 == 0 {
                        // Break when not busy and no error
                        break false;
                    }
                    if start.elapsed() >= TIMEOUT {
                        log::error!("busmaster read timeout with status 0x{:02X}", status);
                        return Err(Error::new(EIO));
                    }
                    thread::yield_now();
                };

                // Stop bus master
                chan.busmaster_command.writef(1, false);

                // Clear bus master error and interrupt
                chan.busmaster_status.write(0b110);

                if error {
                    log::error!("IDE bus master error");
                    return Err(Error::new(EIO));
                }

                // Read buffer
                chunk.copy_from_slice(&chan.buf[..chunk.len()]);
            } else {
                for sector in 0..sectors {
                    chan.polling(true, line!())?;

                    for i in 0..128 {
                        let data = chan.data32.read();
                        chunk[sector * 512 + i * 4 + 0] = (data >> 0) as u8;
                        chunk[sector * 512 + i * 4 + 1] = (data >> 8) as u8;
                        chunk[sector * 512 + i * 4 + 2] = (data >> 16) as u8;
                        chunk[sector * 512 + i * 4 + 3] = (data >> 24) as u8;
                    }
                }
            }

            count += chunk.len();
        }

        Ok(count)
    }

    // NOTE: not async
    async fn write(&mut self, start_block: u64, buffer: &[u8]) -> Result<usize> {
        let mut count = 0;
        for chunk in buffer.chunks(65536) {
            let block = start_block + (count as u64) / 512;

            //TODO: support other LBA modes
            assert!(block < 0x1_0000_0000_0000);

            let sectors = (chunk.len() + 511) / 512;
            assert!(sectors <= 128);

            log::trace!(
                "IDE write chan {} dev {} block {:#x} count {:#x}",
                self.chan_i,
                self.dev,
                block,
                sectors
            );

            let mut chan = self.chan.lock().unwrap();

            if self.dma {
                // Stop bus master
                chan.busmaster_command.writef(1, false);
                // Make PRDT EOT match chunk size
                for i in 0..sectors {
                    chan.prdt[i] = PrdtEntry {
                        phys: (chan.buf.physical() + i * 512).try_into().unwrap(),
                        size: 512,
                        flags: if i + 1 == sectors {
                            1 << 15 // End of table
                        } else {
                            0
                        },
                    };
                }
                // Set PRDT
                let prdt = chan.prdt.physical();
                chan.busmaster_prdt.write(prdt.try_into().unwrap());
                // Set to write
                chan.busmaster_command.writef(1 << 3, false);
                // Clear interrupt and error bits
                chan.busmaster_status.write(0b110);

                // Write buffer
                chan.buf[..chunk.len()].copy_from_slice(chunk);
            }

            // Select drive
            //TODO: upper part of LBA 28
            chan.device_select.write(0xE0 | (self.dev << 4));

            if self.lba_48 {
                // Set high sector count and LBA
                chan.control.writef(0x80, true);
                chan.sector_count.write((sectors >> 8) as u8);
                chan.lba_0.write((block >> 24) as u8);
                chan.lba_1.write((block >> 32) as u8);
                chan.lba_2.write((block >> 40) as u8);
                chan.control.writef(0x80, false);
            }

            // Set low sector count and LBA
            chan.sector_count.write(sectors as u8);
            chan.lba_0.write(block as u8);
            chan.lba_1.write((block >> 8) as u8);
            chan.lba_2.write((block >> 16) as u8);

            // Send command
            chan.command.write(if self.dma {
                if self.lba_48 {
                    AtaCommand::WriteDmaExt as u8
                } else {
                    AtaCommand::WriteDma as u8
                }
            } else {
                if self.lba_48 {
                    AtaCommand::WritePioExt as u8
                } else {
                    AtaCommand::WritePio as u8
                }
            });

            // Write data
            if self.dma {
                // Start bus master
                chan.busmaster_command.writef(1, true);

                // Wait for transaction to finish
                chan.polling(false, line!())?;

                // Wait for bus master to finish
                let start = Instant::now();
                let error = loop {
                    let status = chan.busmaster_status.read();
                    if status & 1 << 1 != 0 {
                        // Break with error status
                        break true;
                    }
                    if status & 1 == 0 {
                        // Break when not busy and no error
                        break false;
                    }
                    if start.elapsed() >= TIMEOUT {
                        log::error!("busmaster write timeout with status 0x{:02X}", status);
                        return Err(Error::new(EIO));
                    }
                    thread::yield_now();
                };

                // Stop bus master
                chan.busmaster_command.writef(1, false);

                // Clear bus master error and interrupt
                chan.busmaster_status.write(0b110);

                if error {
                    log::error!("IDE bus master error");
                    return Err(Error::new(EIO));
                }
            } else {
                for sector in 0..sectors {
                    chan.polling(false, line!())?;

                    for i in 0..128 {
                        chan.data32.write(
                            ((chunk[sector * 512 + i * 4 + 0] as u32) << 0)
                                | ((chunk[sector * 512 + i * 4 + 1] as u32) << 8)
                                | ((chunk[sector * 512 + i * 4 + 2] as u32) << 16)
                                | ((chunk[sector * 512 + i * 4 + 3] as u32) << 24),
                        );
                    }
                }
            }

            chan.command.write(if self.lba_48 {
                AtaCommand::CacheFlushExt as u8
            } else {
                AtaCommand::CacheFlush as u8
            });
            chan.polling(false, line!())?;

            count += chunk.len();
        }

        Ok(count)
    }
}
