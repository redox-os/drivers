#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::io::Pio;
use redox_scheme::scheme::SchemeSync;
use redox_scheme::CallerCtx;
use redox_scheme::OpenResult;
use syscall::error::{Error, Result, EACCES, EBADF, EINVAL, ENOENT};
use syscall::schemev2::NewFdFlags;
use syscall::EWOULDBLOCK;

use common::{
    dma::Dma,
    io::{Io, Mmio},
};
use spin::Mutex;

const NUM_SUB_BUFFS: usize = 32;
const SUB_BUFF_SIZE: usize = 2048;

enum Handle {
    Todo,
}

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
    /* 0x28 */ extended_id: Pio<u16>,
    /* 0x2A */ extended_ctrl: Pio<u16>,
    /* 0x2C */ vra_pcm_front: Pio<u16>,
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
            extended_id: Pio::new(bar0 + 0x28),
            extended_ctrl: Pio::new(bar0 + 0x2A),
            vra_pcm_front: Pio::new(bar0 + 0x2C),
        }
    }
}

#[allow(dead_code)]
struct BusBoxRegs {
    /// Buffer descriptor list base address
    /* 0x00 */
    bdbar: Pio<u32>,
    /// Current index value
    /* 0x04 */
    civ: Pio<u8>,
    /// Last valid index
    /* 0x05 */
    lvi: Pio<u8>,
    /// Status
    /* 0x06 */
    sr: Pio<u16>,
    /// Position in current buffer
    /* 0x08 */
    picb: Pio<u16>,
    /// Prefetched index value
    /* 0x0A */
    piv: Pio<u8>,
    /// Control
    /* 0x0B */
    cr: Pio<u8>,
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

#[allow(dead_code)]
struct BusRegs {
    /// PCM in register box
    /* 0x00 */
    pi: BusBoxRegs,
    /// PCM out register box
    /* 0x10 */
    po: BusBoxRegs,
    /// Microphone register box
    /* 0x20 */
    mc: BusBoxRegs,
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

#[repr(C, packed)]
pub struct BufferDescriptor {
    /* 0x00 */ addr: Mmio<u32>,
    /* 0x04 */ samples: Mmio<u16>,
    /* 0x06 */ flags: Mmio<u16>,
}

pub struct Ac97 {
    mixer: MixerRegs,
    bus: BusRegs,
    bdl: Dma<[BufferDescriptor; NUM_SUB_BUFFS]>,
    buf: Dma<[u8; NUM_SUB_BUFFS * SUB_BUFF_SIZE]>,
    handles: Mutex<BTreeMap<usize, Handle>>,
    next_id: AtomicUsize,
}

impl Ac97 {
    pub unsafe fn new(bar0: u16, bar1: u16) -> Result<Self> {
        let mut module = Ac97 {
            mixer: MixerRegs::new(bar0),
            bus: BusRegs::new(bar1),
            bdl: Dma::zeroed(
                //TODO: PhysBox::new_in_32bit_space(bdl_size)?
            )?
            .assume_init(),
            buf: Dma::zeroed(
                //TODO: PhysBox::new_in_32bit_space(buf_size)?
            )?
            .assume_init(),
            handles: Mutex::new(BTreeMap::new()),
            next_id: AtomicUsize::new(0),
        };

        module.init()?;

        Ok(module)
    }

    fn init(&mut self) -> Result<()> {
        //TODO: support other sample rates, or just the default of 48000 Hz
        {
            // Check if VRA is supported
            if !self.mixer.extended_id.readf(1 << 0) {
                println!("ac97d: VRA not supported and is currently required");
                return Err(Error::new(ENOENT));
            }

            // Enable VRA
            self.mixer.extended_ctrl.writef(1 << 0, true);

            // Attempt to set sample rate for PCM front to 44100 Hz
            let desired_sample_rate = 44100;
            self.mixer.vra_pcm_front.write(desired_sample_rate);

            // Read back real sample rate
            let real_sample_rate = self.mixer.vra_pcm_front.read();
            println!("ac97d: set sample rate to {}", real_sample_rate);

            // Error if we cannot set the sample rate as desired
            if real_sample_rate != desired_sample_rate {
                println!(
                    "ac97d: sample rate is {} but only {} is supported",
                    real_sample_rate, desired_sample_rate
                );
                return Err(Error::new(ENOENT));
            }
        }

        // Ensure PCM out is stopped
        self.bus.po.cr.writef(1, false);

        // Reset PCM out
        self.bus.po.cr.writef(1 << 1, true);
        while self.bus.po.cr.readf(1 << 1) {
            // Spinning on resetting PCM out
            //TODO: relax
        }

        // Initialize BDL for PCM out
        for i in 0..NUM_SUB_BUFFS {
            self.bdl[i]
                .addr
                .write((self.buf.physical() + i * SUB_BUFF_SIZE) as u32);
            self.bdl[i]
                .samples
                .write((SUB_BUFF_SIZE / 2/* Each sample is i16 or 2 bytes */) as u16);
            self.bdl[i]
                .flags
                .write(1 << 15 /* Interrupt on completion */);
        }
        self.bus.po.bdbar.write(self.bdl.physical() as u32);

        // Enable interrupt on completion
        self.bus.po.cr.writef(1 << 4, true);

        // Start bus master
        self.bus.po.cr.writef(1 << 0, true);

        // Set master volume to 0 db (loudest output, DANGER!)
        self.mixer.master_volume.write(0);

        // Set PCM output volume to 0 db (medium)
        self.mixer.pcm_out_volume.write(0x808);

        Ok(())
    }

    pub fn irq(&mut self) -> bool {
        let ints = self.bus.po.sr.read() & 0b11100;
        if ints != 0 {
            self.bus.po.sr.write(ints);
            true
        } else {
            false
        }
    }
}

impl SchemeSync for Ac97 {
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
        id: usize,
        buf: &[u8],
        _offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        {
            let mut handles = self.handles.lock();
            let _handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;
        }

        if buf.len() != SUB_BUFF_SIZE {
            return Err(Error::new(EINVAL));
        }

        let civ = self.bus.po.civ.read() as usize;
        let mut lvi = self.bus.po.lvi.read() as usize;
        if lvi == (civ + 3) % NUM_SUB_BUFFS {
            // Block if we already are 3 buffers ahead
            Err(Error::new(EWOULDBLOCK))
        } else {
            // Fill next buffer
            lvi = (lvi + 1) % NUM_SUB_BUFFS;
            for i in 0..SUB_BUFF_SIZE {
                self.buf[lvi * SUB_BUFF_SIZE + i] = buf[i];
            }
            self.bus.po.lvi.write(lvi as u8);

            Ok(SUB_BUFF_SIZE)
        }
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
