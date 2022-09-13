use std::{
    convert::TryInto,
    sync::{Arc, Mutex},
};
use syscall::{
    error::{Error, Result, EIO},
    io::{Dma, Io, Pio, PhysBox, ReadOnly, WriteOnly},
};

use crate::ata::AtaCommand;

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) unsafe fn pause() { std::arch::aarch64::__yield(); }

#[cfg(target_arch = "x86")]
#[inline(always)]
pub(crate) unsafe fn pause() { std::arch::x86::_mm_pause(); }

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) unsafe fn pause() { std::arch::x86_64::_mm_pause(); }

#[repr(packed)]
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
    prdt: Dma<[PrdtEntry; 16]>,
    buf: Dma<[u8; 16 * 4096]>,
}

impl Channel {
    pub fn new(base: u16, control_base: u16, busmaster_base: u16) -> Result<Self> {
        let mut chan = Self {
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
                Dma::from_physbox_zeroed(
                    //TODO: PhysBox::new_in_32bit_space(4096)?
                    PhysBox::new(4096)?
                )?.assume_init()
            },
            buf: unsafe {
                Dma::from_physbox_zeroed(
                    //TODO: PhysBox::new_in_32bit_space(16 * 4096)?
                    PhysBox::new(16 * 4096)?
                )?.assume_init()
            },
        };

        for i in 0..chan.prdt.len() {
            chan.prdt[i] = PrdtEntry {
                phys: (chan.buf.physical() + i * 4096).try_into().unwrap(),
                size: 4096,
                flags: if i + 1 == chan.prdt.len() {
                    1 << 15 // End of table
                } else {
                    0
                },
            };
        }

        Ok(chan)
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

    fn polling(&mut self, read: bool) -> Result<()> {
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

        loop {
            let status = self.check_status()?;
            if status & 0x80 != 0 {
                unsafe { pause(); }
            } else {
                if read && status & 0x08 == 0 {
                    log::error!("IDE data not ready");
                    return Err(Error::new(EIO));
                }
                break;
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
    pub dma: bool,
    pub lba_48: bool,
}

impl Disk for AtaDisk {
    fn id(&self) -> usize {
        self.chan_i << 1 | self.dev as usize
    }

    fn size(&mut self) -> u64 {
        self.size
    }

    fn read(&mut self, start_block: u64, buffer: &mut [u8]) -> Result<Option<usize>> {
        let mut count = 0;
        for chunk in buffer.chunks_mut(65536) {
            let block = start_block + (count as u64) / 512;

            //TODO: support other LBA modes
            assert!(block < 0x1_0000_0000_0000);

            let sectors = chunk.len() / 512;
            assert!(sectors < 0x1_0000);

            let mut chan = self.chan.lock().unwrap();

            if self.dma {
                // Stop bus master
                chan.busmaster_command.writef(1, false);
                // Set PRDT
                let prdt = chan.prdt.physical();
                chan.busmaster_prdt.write(prdt.try_into().unwrap());
                // Set to read
                chan.busmaster_command.writef(1 << 3, true);
                // Clear interrupt and error bits
                chan.busmaster_status.write(0b110);
            }

            // Select drive
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
                chan.polling(false)?;

                //TODO: use bus master status

                // Stop bus master
                chan.busmaster_command.writef(1, false);

                // Read buffer
                chunk.copy_from_slice(&chan.buf[..chunk.len()]);
            } else {
                for sector in 0..sectors {
                    chan.polling(true)?;

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

        Ok(Some(count))
    }

    fn write(&mut self, start_block: u64, buffer: &[u8]) -> Result<Option<usize>> {
        let mut count = 0;
        for chunk in buffer.chunks(65536) {
            let block = start_block + (count as u64) / 512;

            //TODO: support other LBA modes
            assert!(block < 0x1_0000_0000_0000);

            let sectors = chunk.len() / 512;
            assert!(sectors < 0x1_0000);

            let mut chan = self.chan.lock().unwrap();

            if self.dma {
                // Stop bus master
                chan.busmaster_command.writef(1, false);
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
                chan.polling(false)?;

                //TODO: use bus master status

                // Stop bus master
                chan.busmaster_command.writef(1, false);

                chan.check_status()?;
            } else {
                for sector in 0..sectors {
                    chan.polling(false)?;

                    for i in 0..128 {
                        chan.data32.write(
                            ((chunk[sector * 512 + i * 4 + 0] as u32) << 0) |
                            ((chunk[sector * 512 + i * 4 + 1] as u32) << 8) |
                            ((chunk[sector * 512 + i * 4 + 2] as u32) << 16) |
                            ((chunk[sector * 512 + i * 4 + 3] as u32) << 24)
                        );
                    }
                }
            }

            chan.command.write(if self.lba_48 {
                AtaCommand::CacheFlushExt as u8
            } else {
                AtaCommand::CacheFlush as u8
            });
            chan.polling(false)?;

            count += chunk.len();
        }

        Ok(Some(count))
    }

    fn block_length(&mut self) -> Result<u32> {
        Ok(512)
    }
}
