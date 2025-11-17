#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{thread, time};

use common::io::{Io, Pio, ReadOnly, WriteOnly};

use redox_scheme::scheme::SchemeSync;
use redox_scheme::CallerCtx;
use redox_scheme::OpenResult;
use syscall::error::{Error, Result, EACCES, EBADF, ENODEV};
use syscall::schemev2::NewFdFlags;

use spin::Mutex;

const NUM_SUB_BUFFS: usize = 32;
const SUB_BUFF_SIZE: usize = 2048;

enum Handle {
    Todo,
}

#[allow(dead_code)]
pub struct Sb16 {
    handles: Mutex<BTreeMap<usize, Handle>>,
    next_id: AtomicUsize,
    pub(crate) irqs: Vec<u8>,
    dmas: Vec<u8>,
    // Regs
    /* 0x04 */ mixer_addr: WriteOnly<Pio<u8>>,
    /* 0x05 */ mixer_data: Pio<u8>,
    /* 0x06 */ dsp_reset: WriteOnly<Pio<u8>>,
    /* 0x0A */ dsp_read_data: ReadOnly<Pio<u8>>,
    /* 0x0C */ dsp_write_data: WriteOnly<Pio<u8>>,
    /* 0x0C */ dsp_write_status: ReadOnly<Pio<u8>>,
    /* 0x0E */ dsp_read_status: ReadOnly<Pio<u8>>,
}

impl Sb16 {
    pub unsafe fn new(addr: u16) -> Result<Self> {
        let mut module = Sb16 {
            handles: Mutex::new(BTreeMap::new()),
            next_id: AtomicUsize::new(0),
            irqs: Vec::new(),
            dmas: Vec::new(),
            // Regs
            mixer_addr: WriteOnly::new(Pio::new(addr + 0x04)),
            mixer_data: Pio::new(addr + 0x05),
            dsp_reset: WriteOnly::new(Pio::new(addr + 0x06)),
            dsp_read_data: ReadOnly::new(Pio::new(addr + 0x0A)),
            dsp_write_data: WriteOnly::new(Pio::new(addr + 0x0C)),
            dsp_write_status: ReadOnly::new(Pio::new(addr + 0x0C)),
            dsp_read_status: ReadOnly::new(Pio::new(addr + 0x0E)),
        };

        module.init()?;

        Ok(module)
    }

    fn mixer_read(&mut self, index: u8) -> u8 {
        self.mixer_addr.write(index);
        self.mixer_data.read()
    }

    fn mixer_write(&mut self, index: u8, value: u8) {
        self.mixer_addr.write(index);
        self.mixer_data.write(value);
    }

    fn dsp_read(&mut self) -> Result<u8> {
        // Bit 7 must be 1 before data can be sent
        while !self.dsp_read_status.readf(1 << 7) {
            //TODO: timeout!
            std::thread::yield_now();
        }

        Ok(self.dsp_read_data.read())
    }

    fn dsp_write(&mut self, value: u8) -> Result<()> {
        // Bit 7 must be 0 before data can be sent
        while self.dsp_write_status.readf(1 << 7) {
            //TODO: timeout!
            std::thread::yield_now();
        }

        self.dsp_write_data.write(value);
        Ok(())
    }

    fn init(&mut self) -> Result<()> {
        // Perform DSP reset
        {
            // Write 1 to reset port
            self.dsp_reset.write(1);

            // Wait 3us
            thread::sleep(time::Duration::from_micros(3));

            // Write 0 to reset port
            self.dsp_reset.write(0);

            //TODO: Wait for ready byte (0xAA) using read status
            thread::sleep(time::Duration::from_micros(100));

            let ready = self.dsp_read()?;
            if ready != 0xAA {
                log::error!("ready byte was 0x{:02X} instead of 0xAA", ready);
                return Err(Error::new(ENODEV));
            }
        }

        // Read DSP version
        {
            self.dsp_write(0xE1)?;

            let major = self.dsp_read()?;
            let minor = self.dsp_read()?;
            log::info!("DSP version {}.{:02}", major, minor);

            if major != 4 {
                log::error!("Unsupported DSP major version {}", major);
                return Err(Error::new(ENODEV));
            }
        }

        // Get available IRQs and DMAs
        {
            self.irqs.clear();
            let irq_mask = self.mixer_read(0x80);
            if (irq_mask & (1 << 0)) != 0 {
                self.irqs.push(2);
            }
            if (irq_mask & (1 << 1)) != 0 {
                self.irqs.push(5);
            }
            if (irq_mask & (1 << 2)) != 0 {
                self.irqs.push(7);
            }
            if (irq_mask & (1 << 3)) != 0 {
                self.irqs.push(10);
            }

            self.dmas.clear();
            let dma_mask = self.mixer_read(0x81);
            if (dma_mask & (1 << 0)) != 0 {
                self.dmas.push(0);
            }
            if (dma_mask & (1 << 1)) != 0 {
                self.dmas.push(1);
            }
            if (dma_mask & (1 << 3)) != 0 {
                self.dmas.push(3);
            }
            if (dma_mask & (1 << 5)) != 0 {
                self.dmas.push(5);
            }
            if (dma_mask & (1 << 6)) != 0 {
                self.dmas.push(6);
            }
            if (dma_mask & (1 << 7)) != 0 {
                self.dmas.push(7);
            }

            log::info!("IRQs {:02X?} DMAs {:02X?}", self.irqs, self.dmas);
        }

        // Set output sample rate to 44100 Hz (Redox OS standard)
        {
            let rate = 44100u16;
            self.dsp_write(0x41)?;
            self.dsp_write((rate >> 8) as u8)?;
            self.dsp_write(rate as u8)?;
        }

        Ok(())
    }

    pub fn irq(&mut self) -> bool {
        //TODO
        false
    }
}

impl SchemeSync for Sb16 {
    fn open(&mut self, _path: &str, _flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid == 0 {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.handles.lock().insert(id, Handle::Todo);
            Ok(OpenResult::ThisScheme {
                number: id,
                flags: NewFdFlags::empty(),
            })
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn write(
        &mut self,
        _id: usize,
        _buf: &[u8],
        _offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        //TODO
        Err(Error::new(EBADF))
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        let mut handles = self.handles.lock();
        let _handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;
        let scheme_path = b"/scheme/audiohw";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(i)
    }

    fn on_close(&mut self, id: usize) {
        let _ = self.handles.lock().remove(&id);
    }
}
