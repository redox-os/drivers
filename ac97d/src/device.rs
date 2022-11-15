#![allow(dead_code)]

use std::cmp;
use std::collections::HashMap;
use std::str;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use syscall::{PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::error::{Error, EACCES, EBADF, Result, EINVAL};
use syscall::flag::{SEEK_SET, SEEK_CUR, SEEK_END};
use syscall::io::{Pio, Io};
use syscall::scheme::SchemeBlockMut;

use spin::Mutex;

const NUM_SUB_BUFFS: usize = 4;
const SUB_BUFF_SIZE: usize = 2048;

enum Handle {
	Todo,
}

#[repr(packed)]
#[allow(dead_code)]
struct MixerRegs {
	/* 0x00 */ reset: Pio<u16>,
	/* 0x02 */ master_volume: Pio<u16>,
	/* 0x04 */ aux_out_volume: Pio<u16>,
	/* 0x06 */ mono_volume: Pio<u16>,
	/* 0x08 */ master_tone: Pio<u16>,
	/* 0x0A */ pc_beep_volume: Pio<u16>,
	/* 0x0C */ phone_volume: Pio<u16>,
	/* 0x0E */ mic_volume: Pio<u16>,
	/* 0x10 */ line_in_volume: Pio<u16>,
	/* 0x12 */ cd_volume: Pio<u16>,
	/* 0x14 */ video_volume: Pio<u16>,
	/* 0x16 */ aux_in_volume: Pio<u16>,
	/* 0x18 */ pcm_out_volume: Pio<u16>,
	/* 0x1A */ record_select: Pio<u16>,
	/* 0x1C */ record_gain: Pio<u16>,
	/* 0x1E */ record_gain_mic: Pio<u16>,
	/* 0x20 */ general_purpose: Pio<u16>,
	/* 0x22 */ control_3d: Pio<u16>,
	/* 0x24 */ audio_int_paging: Pio<u16>,
	/* 0x26 */ powerdown: Pio<u16>,
	//TODO: extended registers
}

impl MixerRegs {
	fn new(bar0: u16) -> Self {
		Self {
			reset: Pio::new(bar0 + 0x00),
			master_volume: Pio::new(bar0 + 0x02),
			aux_out_volume: Pio::new(bar0 + 0x04),
			mono_volume: Pio::new(bar0 + 0x06),
			master_tone: Pio::new(bar0 + 0x08),
			pc_beep_volume: Pio::new(bar0 + 0x0A),
			phone_volume: Pio::new(bar0 + 0x0C),
			mic_volume: Pio::new(bar0 + 0x0E),
			line_in_volume: Pio::new(bar0 + 0x10),
			cd_volume: Pio::new(bar0 + 0x12),
			video_volume: Pio::new(bar0 + 0x14),
			aux_in_volume: Pio::new(bar0 + 0x16),
			pcm_out_volume: Pio::new(bar0 + 0x18),
			record_select: Pio::new(bar0 + 0x1A),
			record_gain: Pio::new(bar0 + 0x1C),
			record_gain_mic: Pio::new(bar0 + 0x1E),
			general_purpose: Pio::new(bar0 + 0x20),
			control_3d: Pio::new(bar0 + 0x22),
			audio_int_paging: Pio::new(bar0 + 0x24),
			powerdown: Pio::new(bar0 + 0x26),
		}
	}
}

#[repr(packed)]
#[allow(dead_code)]
struct BusBoxRegs {
	/// Buffer descriptor list base address
	/* 0x00 */ bdbar: Pio<u32>,
	/// Current index value
	/* 0x04 */ civ: Pio<u8>,
	/// Last valid index
	/* 0x05 */ lvi: Pio<u8>,
	/// Status
	/* 0x06 */ sr: Pio<u16>,
	/// Position in current buffer
	/* 0x08 */ picb: Pio<u16>,
	/// Prefetched index value
	/* 0x0A */ piv: Pio<u8>,
	/// Control
	/* 0x0B */ cr: Pio<u8>,
}

impl BusBoxRegs {
	fn new(base: u16) -> Self {
		Self {
			bdbar: Pio::new(base + 0x00),
			civ: Pio::new(base + 0x04),
			lvi: Pio::new(base + 0x05),
			sr: Pio::new(base + 0x06),
			picb: Pio::new(base + 0x08),
			piv: Pio::new(base + 0x0A),
			cr: Pio::new(base + 0x0B),
		}
	}
}

#[repr(packed)]
#[allow(dead_code)]
struct BusRegs {
	/// PCM in register box
	/* 0x00 */ pi: BusBoxRegs,
	/// PCM out register box
	/* 0x10 */ po: BusBoxRegs,
	/// Microphone register box
	/* 0x20 */ mc: BusBoxRegs,
}

impl BusRegs {
	fn new(bar1: u16) -> Self {
		Self {
			pi: BusBoxRegs::new(bar1 + 0x00),
			po: BusBoxRegs::new(bar1 + 0x10),
			mc: BusBoxRegs::new(bar1 + 0x20),
		}
	}
}

pub struct Ac97 {
	mixer: MixerRegs,
	bus: BusRegs,
	handles: Mutex<BTreeMap<usize, Handle>>,
	next_id: AtomicUsize,
}

impl Ac97 {
	pub unsafe fn new(bar0: u16, bar1: u16) -> Result<Self> {
		let mut module = Ac97 {
			mixer: MixerRegs::new(bar0),
			bus: BusRegs::new(bar1),
			handles: Mutex::new(BTreeMap::new()),
			next_id: AtomicUsize::new(0),
		};

		//TODO: init

		Ok(module)
	}

	pub fn irq(&mut self) -> bool {
		//TODO
		false
	}
}

impl SchemeBlockMut for Ac97 {
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
		let index = {
	        let mut handles = self.handles.lock();
	        let _handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;
			0
		};

		Ok(Some(buf.len()))
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
