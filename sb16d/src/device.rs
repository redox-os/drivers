#![allow(dead_code)]

use std::{mem, thread, time};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use syscall::error::{Error, EACCES, EBADF, Result, EINVAL, ENODEV, ENOENT};
use syscall::io::{Dma, PhysBox, Mmio, Pio, Io, ReadOnly, WriteOnly};
use syscall::scheme::SchemeBlockMut;

use spin::Mutex;

const NUM_SUB_BUFFS: usize = 32;
const SUB_BUFF_SIZE: usize = 2048;

enum Handle {
	Todo,
}

#[allow(dead_code)]
struct DspRegs {
	/* 0x06 */ reset: WriteOnly<Pio<u8>>,
	/* 0x0A */ read_data: ReadOnly<Pio<u8>>,
	/* 0x0C */ write_data: WriteOnly<Pio<u8>>,
	/* 0x0C */ write_status: ReadOnly<Pio<u8>>,
	/* 0x0E */ read_status: ReadOnly<Pio<u8>>,
}

impl DspRegs {
	fn new(addr: u16) -> Self {
		Self {
			reset: WriteOnly::new(Pio::new(addr + 0x06)),
			read_data: ReadOnly::new(Pio::new(addr + 0x0A)),
			write_data: WriteOnly::new(Pio::new(addr + 0x0C)),
			write_status: ReadOnly::new(Pio::new(addr + 0x0C)),
			read_status: ReadOnly::new(Pio::new(addr + 0x0E)),
		}
	}
}

pub struct Sb16 {
	dsp: DspRegs,
	handles: Mutex<BTreeMap<usize, Handle>>,
	next_id: AtomicUsize,
}

impl Sb16 {
	pub unsafe fn new(addr: u16) -> Result<Self> {
		let mut module = Sb16 {
			dsp: DspRegs::new(addr),
			handles: Mutex::new(BTreeMap::new()),
			next_id: AtomicUsize::new(0),
		};

		module.init()?;

		Ok(module)
	}

	fn init(&mut self) -> Result<()> {
		// Perform DSP reset
		{
			// Write 1 to reset port
			self.dsp.reset.write(1);

			// Wait 3us
			thread::sleep(time::Duration::from_micros(3));

			// Write 0 to reset port
			self.dsp.reset.write(0);

			//TODO: Wait for ready byte (0xAA) using read status
			thread::sleep(time::Duration::from_micros(100));

			let ready = self.dsp.read_data.read();
			if ready != 0xAA {
				log::error!("ready byte was 0x{:02X} instead of 0xAA", ready);
				return Err(Error::new(ENODEV));
			}
		}

		// Read DSP version
		{
			//TODO
		}

		Ok(())
	}

	pub fn irq(&mut self) -> bool {
		//TODO
		false
	}
}

impl SchemeBlockMut for Sb16 {
	fn open(&mut self, _path: &str, _flags: usize, uid: u32, _gid: u32) -> Result<Option<usize>> {
		if uid == 0 {
			let id = self.next_id.fetch_add(1, Ordering::SeqCst);
			self.handles.lock().insert(id, Handle::Todo);
			Ok(Some(id))
		} else {
			Err(Error::new(EACCES))
		}
	}

	fn write(&mut self, id: usize, buf: &[u8]) -> Result<Option<usize>> {
		//TODO
		Err(Error::new(EBADF))
	}

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        let mut handles = self.handles.lock();
        let _handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;
        let scheme_path = b"audiohw:";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(Some(i))
    }

	fn close(&mut self, id: usize) -> Result<Option<usize>> {
		let mut handles = self.handles.lock();
    	handles.remove(&id).ok_or(Error::new(EBADF)).and(Ok(Some(0)))
	}
}
