use common::dma::Dma;
use common::io::{Io, Mmio};
use common::timeout::Timeout;
use syscall::error::{Error, Result, EIO};

use super::common::*;

// CORBCTL
const CMEIE: u8 = 1 << 0; // 1 bit
const CORBRUN: u8 = 1 << 1; // 1 bit

// CORBSIZE
const CORBSZCAP: (u8, u8) = (4, 4);
const CORBSIZE: (u8, u8) = (0, 2);

// CORBRP
const CORBRPRST: u16 = 1 << 15;

// RIRBWP
const RIRBWPRST: u16 = 1 << 15;

// RIRBCTL
const RINTCTL: u8 = 1 << 0; // 1 bit
const RIRBDMAEN: u8 = 1 << 1; // 1 bit

const CORB_OFFSET: usize = 0x00;
const RIRB_OFFSET: usize = 0x10;
const ICMD_OFFSET: usize = 0x20;

// ICS
const ICB: u16 = 1 << 0;
const IRV: u16 = 1 << 1;

// CORB and RIRB offset

const COMMAND_BUFFER_OFFSET: usize = 0x40;
const CORB_BUFF_MAX_SIZE: usize = 1024;

struct CommandBufferRegs {
    corblbase: Mmio<u32>,
    corbubase: Mmio<u32>,
    corbwp: Mmio<u16>,
    corbrp: Mmio<u16>,
    corbctl: Mmio<u8>,
    corbsts: Mmio<u8>,
    corbsize: Mmio<u8>,
    rsvd5: Mmio<u8>,

    rirblbase: Mmio<u32>,
    rirbubase: Mmio<u32>,
    rirbwp: Mmio<u16>,
    rintcnt: Mmio<u16>,
    rirbctl: Mmio<u8>,
    rirbsts: Mmio<u8>,
    rirbsize: Mmio<u8>,
    rsvd6: Mmio<u8>,
}

struct CorbRegs {
    corblbase: Mmio<u32>,
    corbubase: Mmio<u32>,
    corbwp: Mmio<u16>,
    corbrp: Mmio<u16>,
    corbctl: Mmio<u8>,
    corbsts: Mmio<u8>,
    corbsize: Mmio<u8>,
    rsvd5: Mmio<u8>,
}

struct Corb {
    regs: &'static mut CorbRegs,
    corb_base: *mut u32,
    corb_base_phys: usize,
    corb_count: usize,
}

impl Corb {
    pub fn new(regs_addr: usize, corb_buff_phys: usize, corb_buff_virt: *mut u32) -> Corb {
        unsafe {
            Corb {
                regs: &mut *(regs_addr as *mut CorbRegs),
                corb_base: corb_buff_virt,
                corb_base_phys: corb_buff_phys,
                corb_count: 0,
            }
        }
    }

    //Intel 4.4.1.3
    pub fn init(&mut self) -> Result<()> {
        self.stop()?;
        //Determine CORB and RIRB size and allocate buffer

        //3.3.24
        let corbsize_reg = self.regs.corbsize.read();
        let corbszcap = (corbsize_reg >> 4) & 0xF;

        let mut corbsize_bytes: usize = 0;
        let mut corbsize: u8 = 0;

        if (corbszcap & 4) == 4 {
            corbsize = 2;
            corbsize_bytes = 1024;

            self.corb_count = 256;
        } else if (corbszcap & 2) == 2 {
            corbsize = 1;
            corbsize_bytes = 64;

            self.corb_count = 16;
        } else if (corbszcap & 1) == 1 {
            corbsize = 0;
            corbsize_bytes = 8;

            self.corb_count = 2;
        }

        assert!(self.corb_count != 0);
        let addr = self.corb_base_phys;
        self.set_address(addr);
        self.regs.corbsize.write((corbsize_reg & 0xFC) | corbsize);

        self.reset_read_pointer()?;
        let old_wp = self.regs.corbwp.read();
        self.regs.corbwp.write(old_wp & 0xFF00);

        Ok(())
    }

    pub fn start(&mut self) {
        self.regs.corbctl.writef(CORBRUN, true);
    }

    #[inline(never)]
    pub fn stop(&mut self) -> Result<()> {
        let timeout = Timeout::from_secs(1);
        while self.regs.corbctl.readf(CORBRUN) {
            self.regs.corbctl.writef(CORBRUN, false);
            timeout.run().map_err(|()| {
                log::error!("timeout on clearing CORBRUN");
                Error::new(EIO)
            })?;
        }
        Ok(())
    }

    pub fn set_address(&mut self, addr: usize) {
        self.regs.corblbase.write((addr & 0xFFFFFFFF) as u32);
        self.regs.corbubase.write(((addr as u64) >> 32) as u32);
    }

    pub fn reset_read_pointer(&mut self) -> Result<()> {
        // 3.3.21

        self.stop()?;

        // Set CORBRPRST to 1
        log::trace!("CORBRP {:X}", self.regs.corbrp.read());
        self.regs.corbrp.writef(CORBRPRST, true);
        log::trace!("CORBRP {:X}", self.regs.corbrp.read());

        {
            // Wait for it to become 1
            let timeout = Timeout::from_secs(1);
            while !self.regs.corbrp.readf(CORBRPRST) {
                self.regs.corbrp.writef(CORBRPRST, true);
                timeout.run().map_err(|()| {
                    log::error!("timeout on setting CORBRPRST");
                    Error::new(EIO)
                })?;
            }
        }

        // Clear the bit again
        self.regs.corbrp.writef(CORBRPRST, false);

        {
            // Read back the bit until zero to verify that it is cleared.
            let timeout = Timeout::from_secs(1);
            loop {
                if !self.regs.corbrp.readf(CORBRPRST) {
                    break;
                }
                self.regs.corbrp.writef(CORBRPRST, false);
                timeout.run().map_err(|()| {
                    log::error!("timeout on clearing CORBRPRST");
                    Error::new(EIO)
                })?;
            }
        }

        Ok(())
    }

    fn send_command(&mut self, cmd: u32) -> Result<()> {
        {
            // wait for the commands to finish
            let timeout = Timeout::from_secs(1);
            while (self.regs.corbwp.read() & 0xff) != (self.regs.corbrp.read() & 0xff) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on CORB command");
                    Error::new(EIO)
                })?;
            }
        }
        let write_pos: usize = ((self.regs.corbwp.read() as usize & 0xFF) + 1) % self.corb_count;
        unsafe {
            *self.corb_base.offset(write_pos as isize) = cmd;
        }

        self.regs.corbwp.write(write_pos as u16);

        log::trace!("Corb: {:08X}", cmd);
        Ok(())
    }
}

struct RirbRegs {
    rirblbase: Mmio<u32>,
    rirbubase: Mmio<u32>,
    rirbwp: Mmio<u16>,
    rintcnt: Mmio<u16>,
    rirbctl: Mmio<u8>,
    rirbsts: Mmio<u8>,
    rirbsize: Mmio<u8>,
    rsvd6: Mmio<u8>,
}

struct Rirb {
    regs: &'static mut RirbRegs,
    rirb_base: *mut u64,
    rirb_base_phys: usize,
    rirb_rp: u16,
    rirb_count: usize,
}

impl Rirb {
    pub fn new(regs_addr: usize, rirb_buff_phys: usize, rirb_buff_virt: *mut u64) -> Rirb {
        unsafe {
            Rirb {
                regs: &mut *(regs_addr as *mut RirbRegs),
                rirb_base: rirb_buff_virt,
                rirb_rp: 0,
                rirb_base_phys: rirb_buff_phys,
                rirb_count: 0,
            }
        }
    }
    //Intel 4.4.1.3
    pub fn init(&mut self) -> Result<()> {
        self.stop()?;

        let rirbsize_reg = self.regs.rirbsize.read();
        let rirbszcap = (rirbsize_reg >> 4) & 0xF;

        let mut rirbsize_bytes: usize = 0;
        let mut rirbsize: u8 = 0;

        if (rirbszcap & 4) == 4 {
            rirbsize = 2;
            rirbsize_bytes = 2048;

            self.rirb_count = 256;
        } else if (rirbszcap & 2) == 2 {
            rirbsize = 1;
            rirbsize_bytes = 128;

            self.rirb_count = 8;
        } else if (rirbszcap & 1) == 1 {
            rirbsize = 0;
            rirbsize_bytes = 16;

            self.rirb_count = 2;
        }

        assert!(self.rirb_count != 0);

        let addr = self.rirb_base_phys;
        self.set_address(addr);

        self.reset_write_pointer();
        self.rirb_rp = 0;

        self.regs.rintcnt.write(1);

        Ok(())
    }

    pub fn start(&mut self) {
        self.regs.rirbctl.writef(RIRBDMAEN | RINTCTL, true);
    }

    pub fn stop(&mut self) -> Result<()> {
        let timeout = Timeout::from_secs(1);
        while self.regs.rirbctl.readf(RIRBDMAEN) {
            self.regs.rirbctl.writef(RIRBDMAEN, false);
            timeout.run().map_err(|()| {
                log::error!("timeout on clearing RIRBDMAEN");
                Error::new(EIO)
            })?;
        }
        Ok(())
    }

    pub fn set_address(&mut self, addr: usize) {
        self.regs.rirblbase.write((addr & 0xFFFFFFFF) as u32);
        self.regs.rirbubase.write(((addr as u64) >> 32) as u32);
    }

    pub fn reset_write_pointer(&mut self) {
        self.regs.rirbwp.writef(RIRBWPRST, true);
    }

    fn read_response(&mut self) -> Result<u64> {
        {
            // wait for response
            let timeout = Timeout::from_secs(1);
            while (self.regs.rirbwp.read() & 0xff) == (self.rirb_rp & 0xff) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on RIRB response");
                    Error::new(EIO)
                })?;
            }
        }
        let read_pos: u16 = (self.rirb_rp + 1) % self.rirb_count as u16;

        let res: u64;
        unsafe {
            res = *self.rirb_base.offset(read_pos as isize);
        }
        self.rirb_rp = read_pos;
        log::trace!("Rirb: {:08X}", res);
        Ok(res)
    }
}

struct ImmediateCommandRegs {
    icoi: Mmio<u32>,
    irii: Mmio<u32>,
    ics: Mmio<u16>,
    rsvd7: [Mmio<u8>; 6],
}

pub struct ImmediateCommand {
    regs: &'static mut ImmediateCommandRegs,
}

impl ImmediateCommand {
    pub fn new(regs_addr: usize) -> ImmediateCommand {
        unsafe {
            ImmediateCommand {
                regs: &mut *(regs_addr as *mut ImmediateCommandRegs),
            }
        }
    }

    pub fn cmd(&mut self, cmd: u32) -> Result<u64> {
        {
            // wait for ready
            let timeout = Timeout::from_secs(1);
            while self.regs.ics.readf(ICB) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on immediate command");
                    Error::new(EIO)
                })?;
            }
        }

        // write command
        self.regs.icoi.write(cmd);

        // set ICB bit to send command
        self.regs.ics.writef(ICB, true);

        {
            // wait for IRV bit to be set to indicate a response is latched
            let timeout = Timeout::from_secs(1);
            while !self.regs.ics.readf(IRV) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on immediate response");
                    Error::new(EIO)
                })?;
            }
        }

        // read the result register twice, total of 8 bytes
        // highest 4 will most likely be zeros (so I've heard)
        let mut res: u64 = self.regs.irii.read() as u64;
        res |= (self.regs.irii.read() as u64) << 32;

        // clear the bit so we know when the next response comes
        self.regs.ics.writef(IRV, false);

        Ok(res)
    }
}

pub struct CommandBuffer {
    // regs: &'static mut CommandBufferRegs,
    corb: Corb,
    rirb: Rirb,
    icmd: ImmediateCommand,

    use_immediate_cmd: bool,
    mem: Dma<[u8; 0x1000]>,
}

impl CommandBuffer {
    pub fn new(regs_addr: usize, mut cmd_buff: Dma<[u8; 0x1000]>) -> CommandBuffer {
        let corb = Corb::new(
            regs_addr + CORB_OFFSET,
            cmd_buff.physical(),
            cmd_buff.as_mut_ptr().cast(),
        );
        let rirb = Rirb::new(
            regs_addr + RIRB_OFFSET,
            cmd_buff.physical() + CORB_BUFF_MAX_SIZE,
            cmd_buff
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(CORB_BUFF_MAX_SIZE)
                .cast(),
        );

        let icmd = ImmediateCommand::new(regs_addr + ICMD_OFFSET);

        let cmdbuff = CommandBuffer {
            corb,
            rirb,
            icmd,

            use_immediate_cmd: false,

            mem: cmd_buff,
        };

        cmdbuff
    }

    pub fn init(&mut self, use_imm_cmds: bool) -> Result<()> {
        self.corb.init()?;
        self.rirb.init()?;
        self.set_use_imm_cmds(use_imm_cmds)?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.corb.stop()?;
        self.rirb.stop()?;
        Ok(())
    }

    pub fn cmd12(&mut self, addr: WidgetAddr, command: u32, data: u8) -> Result<u64> {
        let mut ncmd: u32 = 0;

        ncmd |= (addr.0 as u32 & 0x00F) << 28;
        ncmd |= (addr.1 as u32 & 0x0FF) << 20;
        ncmd |= (command & 0xFFF) << 8;
        ncmd |= (data as u32 & 0x0FF) << 0;
        self.cmd(ncmd)
    }
    pub fn cmd4(&mut self, addr: WidgetAddr, command: u32, data: u16) -> Result<u64> {
        let mut ncmd: u32 = 0;

        ncmd |= (addr.0 as u32 & 0x000F) << 28;
        ncmd |= (addr.1 as u32 & 0x00FF) << 20;
        ncmd |= (command & 0x000F) << 16;
        ncmd |= (data as u32 & 0xFFFF) << 0;
        self.cmd(ncmd)
    }

    pub fn cmd(&mut self, cmd: u32) -> Result<u64> {
        if self.use_immediate_cmd {
            self.cmd_imm(cmd)
        } else {
            self.cmd_buff(cmd)
        }
    }

    pub fn cmd_imm(&mut self, cmd: u32) -> Result<u64> {
        self.icmd.cmd(cmd)
    }

    pub fn cmd_buff(&mut self, cmd: u32) -> Result<u64> {
        self.corb.send_command(cmd)?;
        self.rirb.read_response()
    }

    pub fn set_use_imm_cmds(&mut self, use_imm: bool) -> Result<()> {
        self.use_immediate_cmd = use_imm;

        if self.use_immediate_cmd {
            self.corb.stop()?;
            self.rirb.stop()?;
        } else {
            self.corb.start();
            self.rirb.start();
        }
        Ok(())
    }
}
