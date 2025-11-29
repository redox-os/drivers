use common::io::{Io, Mmio};
use driver_block::Disk;
use std::{sync::RwLock, thread, time::Duration};
use syscall::{Error, Result, EINVAL};

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) unsafe fn wait_cycles(mut n: usize) {
    use core::arch::asm;

    while n > 0 {
        asm!("nop");
        n -= 1;
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) unsafe fn wait_msec(mut n: usize) {
    use core::arch::asm;

    let mut f: usize;
    let mut t: usize;
    let mut r: usize;

    asm!("mrs {0}, cntfrq_el0", out(reg) f);
    asm!("mrs {0}, cntpct_el0", out(reg) t);

    t += ((f / 1000) * n) / 1000;

    loop {
        asm!("mrs {0}, cntpct_el0", out(reg) r);
        if r >= t {
            break;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) unsafe fn wait_msec(n: usize) {
    thread::sleep(Duration::from_millis(n as u64));
}

//cmd Flags
const CMD_NEED_APP: u32 = 0x8000_0000;
const CMD_RSPNS_48: u32 = 0x0002_0000;
const CMD_ERRORS_MASK: u32 = 0xfff9_c004;
const CMD_RCA_MASK: u32 = 0xffff_0000;

//CMD
const CMD_GO_IDLE: u32 = 0x0000_0000;
const CMD_ALL_SEND_CID: u32 = 0x0201_0000;
const CMD_SEND_CSD: u32 = 0x0901_0000;
const CMD_SEND_REL_ADDR: u32 = 0x0302_0000;
const CMD_CARD_SELECT: u32 = 0x0703_0000;
const CMD_SEND_IF_COND: u32 = 0x0802_0000;
const CMD_STOP_TRANS: u32 = 0x0c03_0000;
const CMD_READ_SINGLE: u32 = 0x1122_0010;
const CMD_READ_MULTI: u32 = 0x1222_0032;
const CMD_SET_BLOCKCNT: u32 = 0x1702_0000;
const CMD_WRITE_SINGLE: u32 = 0x1822_0000;
const CMD_WRITE_MULTI: u32 = 0x1922_0022;

const CMD_APP_CMD: u32 = 0x3700_0000;
const CMD_SET_BUS_WIDTH: u32 = 0x0602_0000 | CMD_NEED_APP;
const CMD_SEND_OP_COND: u32 = 0x2902_0000 | CMD_NEED_APP;
const CMD_SEND_SCR: u32 = 0x3322_0010 | CMD_NEED_APP;

//STATUS register settings
const SR_READ_AVAILABLE: u32 = 0x0000_0800;
const SR_WRITE_AVAILABLE: u32 = 0x0000_0400;
const SR_DAT_INHIBIT: u32 = 0x0000_0002;
const SR_CMD_INHIBIT: u32 = 0x0000_0001;
const SR_APP_CMD: u32 = 0x0000_0020;

//CONTROL register settings

const C0_SPI_MODE_EN: u32 = 0x0010_0000;
const C0_HCTL_HS_EN: u32 = 0x0000_0004;
const C0_HCTL_DWITDH: u32 = 0x0000_0002;

const C1_SRST_DATA: u32 = 0x0400_0000;
const C1_SRST_CMD: u32 = 0x0200_0000;
const C1_SRST_HC: u32 = 0x0100_0000;
const C1_TOUNIT_DIS: u32 = 0x000f_0000;
const C1_TOUNIT_MAX: u32 = 0x000e_0000;
const C1_CLK_GENSEL: u32 = 0x0000_0020;
const C1_CLK_EN: u32 = 0x0000_0004;
const C1_CLK_STABLE: u32 = 0x0000_0002;
const C1_CLK_INTLEN: u32 = 0x0000_0001;

//INTERRUPT register settings
const INT_DATA_TIMEOUT: u32 = 0x0010_0000;
const INT_CMD_TIMEOUT: u32 = 0x0001_0000;
const INT_READ_RDY: u32 = 0x0000_0020;
const INT_WRITE_RDY: u32 = 0x0000_0010;
const INT_DATA_DONE: u32 = 0x0000_0002;
const INT_CMD_DONE: u32 = 0x0000_0001;
const INT_ERROR_MASK: u32 = 0x017e_8000;

const HOST_SPEC_VERSION_OFFSET: u32 = 16;
const HOST_SPEC_VERSION_MASK: u32 = 0x00ff_0000;
const HOST_SPEC_V3: u32 = 2;
const HOST_SPEC_V2: u32 = 1;
const HOST_SPEC_V1: u32 = 0;

const ACMD41_VOLTAGE: u32 = 0x00ff_8000;
const ACMD41_CMD_COMPLETE: u32 = 0x8000_0000;
const ACMD41_CMD_CCS: u32 = 0x4000_0000;
const ACMD41_ARG_HC: u32 = 0x51ff_8000;

const SCR_SD_BUS_WIDTH_4: u32 = 0x0000_0400;
const SCR_SUPP_SET_BLKCNT: u32 = 0x0200_0000;
//added by bztsrc driver
const SCR_SUPP_CCS: u32 = 0x0000_0001;

#[repr(C, packed)]
pub struct SdHostCtrlRegs {
    //LSB

    //ACMD23 Argument
    _arg2: Mmio<u32>,

    //Block Size and Count
    blksizecnt: Mmio<u32>,

    //Argument
    arg1: Mmio<u32>,

    //Command and Transfer Mode
    cmdtm: Mmio<u32>,

    //Response bit 0-127
    resp0: Mmio<u32>,
    resp1: Mmio<u32>,
    resp2: Mmio<u32>,
    resp3: Mmio<u32>,

    //Data
    data: Mmio<u32>,

    //Status
    status: Mmio<u32>,

    //Host Configuration bits
    control0: Mmio<u32>,

    //Host Configuration bits
    control1: Mmio<u32>,

    //Interrupt Flags
    interrupt: Mmio<u32>,

    //Interrupt Flag Enable
    irpt_mask: Mmio<u32>,

    //Interrupt Generation Enable
    irpt_en: Mmio<u32>,

    //Host Configuration bits
    _control2: Mmio<u32>,

    _rsvd: [Mmio<u32>; 47],

    //Slot Interrupt Status and Version
    slotisr_ver: Mmio<u32>,
}

//TODO: refactor, sd/sdhci/bcmh2835-sdhci three different modules.
pub struct SdHostCtrl {
    regs: RwLock<&'static mut SdHostCtrlRegs>,
    host_spec_ver: u32,
    cid: [u32; 4],
    csd: [u32; 4],
    rca: u32, //relative card address
    scr: [u32; 2],
    ocr: u32,
    size: u64,
}

impl SdHostCtrl {
    pub fn new(address: usize) -> Self {
        SdHostCtrl {
            regs: RwLock::new(unsafe { &mut *(address as *mut SdHostCtrlRegs) }),
            host_spec_ver: 0,
            cid: [0; 4],
            csd: [0; 4],
            rca: 0,
            scr: [0; 2],
            ocr: 0,
            size: 0,
        }
    }

    pub unsafe fn init(&mut self) {
        let regs = self.regs.get_mut().unwrap();

        let mut reg_val = regs.slotisr_ver.read();
        self.host_spec_ver = (reg_val & HOST_SPEC_VERSION_MASK) >> HOST_SPEC_VERSION_OFFSET;

        regs.control0.write(0x0);
        reg_val = regs.control1.read();
        regs.control1.write(reg_val | C1_SRST_HC);
        let mut cnt = 1000;
        while (cnt >= 0) && ((regs.control1.read() & C1_SRST_HC) == C1_SRST_HC) {
            cnt -= 1;
            wait_msec(10);
        }

        if cnt < 0 {
            println!("ERROR: failed to reset EMMC");
            return;
        }
        println!("EMMC: reset OK");
        reg_val = regs.control1.read();
        regs.control1.write(reg_val | C1_CLK_INTLEN | C1_TOUNIT_MAX);

        wait_msec(10);

        {
            if let Err(_) = self.set_clock(40_0000) {
                println!("ERROR: failed to set clock {}", 40_0000);
                return;
            }
        }

        let regs = self.regs.get_mut().unwrap();
        regs.irpt_en.write(0xffff_ffff);
        regs.irpt_mask.write(0xffff_ffff);

        if let Err(_) = self.sd_cmd(CMD_GO_IDLE, 0) {
            println!("failed to go idle");
            return;
        }

        if let Err(_) = self.sd_cmd(CMD_SEND_IF_COND, 0x0000_01aa) {
            println!("failed to send if cond");
            return;
        }

        cnt = 6;
        reg_val = 0;

        while ((reg_val & ACMD41_CMD_COMPLETE) == 0) && cnt > 0 {
            wait_msec(10);
            cnt -= 1;

            if let Ok(val) = self.sd_cmd(CMD_SEND_OP_COND, ACMD41_ARG_HC) {
                reg_val = val;
                self.ocr = reg_val;
                print!("EMMC: CMD_SEND_OP_COND returned 0x{:08x} = ", reg_val);

                if (reg_val & ACMD41_CMD_COMPLETE) != 0 {
                    print!("COMPLETE ");
                }
                if (reg_val & ACMD41_VOLTAGE) != 0 {
                    print!("VOLTAGE ");
                }
                if (reg_val & ACMD41_CMD_CCS) != 0 {
                    print!("CCS ");
                }
                print!("\n");
            } else {
                println!("ERROR: EMMC ACMD41 returned error");
                return;
            }
        }

        if (reg_val & ACMD41_CMD_COMPLETE) == 0 || cnt <= 0 {
            println!("ACMD41 TIMEOUT");
            return;
        }

        if (reg_val & ACMD41_VOLTAGE) == 0 {
            println!("ACMD41 VOLTAGE NOT FOUND!");
            return;
        }

        let ccs = if (reg_val & ACMD41_CMD_CCS) != 0 {
            SCR_SUPP_CCS
        } else {
            0
        };

        if let Err(_) = self.sd_cmd(CMD_ALL_SEND_CID, 0) {
            println!("CMD_ALL_SEND_CID ERROR, IGNORE!");
        }

        let sd_rca = self.sd_cmd(CMD_SEND_REL_ADDR, 0x0).unwrap();
        println!("CMD_SEND_REL_ADDR = 0x{:08x}", sd_rca);
        self.rca = sd_rca;

        if let Err(_) = self.sd_cmd(CMD_SEND_CSD, sd_rca) {
            println!("failed to get csd");
            return;
        }

        let (csize, cmult) = if (self.ocr & ACMD41_CMD_CCS) != 0 {
            let csize = (self.csd[1] & 0x3f) << 16 | (self.csd[2] & 0xffff_0000) >> 16;
            let cmult = 8;
            (csize as u64, cmult as u64)
        } else {
            let csize = (self.csd[1] & 0x3ff) << 2 | (self.csd[2] & 0xc000_0000) >> 30;
            let cmult = (self.csd[2] & 0x0003_8000) >> 15;
            (csize as u64, cmult as u64)
        };
        self.size = ((csize + 1) << (cmult + 2)) * 512;
        println!("mmc size = 0x{:08x}", self.size);

        if let Err(_) = self.set_clock(2500_0000) {
            println!("failed to set clock 2500_0000 Hz");
            return;
        }

        if let Err(_) = self.sd_cmd(CMD_CARD_SELECT, sd_rca) {
            println!("failed to CMD_CARD_SELECT 0x{:08x}", sd_rca);
            return;
        }

        if let Err(_) = self.sd_status(SR_DAT_INHIBIT) {
            println!("SR_DAT_INHIBIT return");
            return;
        }

        let regs = self.regs.get_mut().unwrap();
        regs.blksizecnt.write(1 << 16 | 8);

        if let Err(_) = self.sd_cmd(CMD_SEND_SCR, 0) {
            println!("failed to CMD_SEND_SCR");
            return;
        }

        if let Err(_) = self.sd_int(INT_READ_RDY) {
            println!("failed to INT_READ_RDY");
            return;
        }

        cnt = 10000;
        let mut i = 0;
        let regs = self.regs.get_mut().unwrap();
        while i < 2 && cnt > 0 {
            reg_val = regs.status.read();
            cnt -= 1;
            if (reg_val & SR_READ_AVAILABLE) != 0 {
                self.scr[i] = regs.data.read();
                i += 1;
            } else {
                wait_msec(10);
                cnt -= 1;
            }
        }
        if i != 2 {
            println!("SD TIMEOUT FOR SCR[; 2]");
            return;
        }

        if (self.scr[0] & SCR_SD_BUS_WIDTH_4) != 0 {
            if let Err(_) = self.sd_cmd(CMD_SET_BUS_WIDTH, sd_rca | 2) {
                println!("failed to set bus width, {}", sd_rca | 2);
                return;
            }
            let regs = self.regs.get_mut().unwrap();
            regs.control0.write(C0_HCTL_DWITDH);
        }

        print!("EMMC: supports ");

        if (self.scr[0] & SCR_SUPP_SET_BLKCNT) != 0 {
            print!("SET_BLKCNT ");
        }

        if ccs != 0 {
            print!("CCS ");
        }

        print!("\n");

        self.scr[0] &= !SCR_SUPP_CCS;
        self.scr[0] |= ccs;
    }

    pub unsafe fn set_clock(&mut self, freq: u32) -> Result<()> {
        let regs = self.regs.get_mut().unwrap();

        let mut reg_val = regs.status.read() & (SR_CMD_INHIBIT | SR_DAT_INHIBIT);
        let mut cnt = 10_0000;
        while (cnt > 0) && reg_val != 0 {
            wait_msec(1);
            cnt -= 1;
            reg_val = regs.status.read() & (SR_CMD_INHIBIT | SR_DAT_INHIBIT);
        }

        if cnt <= 0 {
            println!("ERROR: TIMEOUT WAITING FOR INHIBIT FLAG");
            return Err(Error::new(EINVAL));
        }

        reg_val = regs.control1.read();
        reg_val &= !C1_CLK_EN;
        regs.control1.write(reg_val);
        wait_msec(10);

        let c = 4166_6666 / freq;
        let mut x: u32 = c - 1;
        let mut s: u32 = 32;

        if x == 0 {
            s = 0;
        } else {
            if (x & 0xffff_0000) == 0 {
                x <<= 16;
                s -= 16;
            }
            if (x & 0xff00_0000) == 0 {
                x <<= 8;
                s -= 8;
            }
            if (x & 0xf000_0000) == 0 {
                x <<= 4;
                s -= 4;
            }
            if (x & 0xc000_0000) == 0 {
                x <<= 2;
                s -= 2;
            }
            if (x & 0x8000_0000) == 0 {
                x <<= 1;
                s -= 1;
            }
            if s > 0 {
                s -= 1;
            }
            if s > 7 {
                s = 7;
            }
        }
        let mut d;
        if self.host_spec_ver > HOST_SPEC_V2 {
            d = c;
        } else {
            d = 1 << s;
        }

        if d <= 2 {
            d = 2;
            s = 0;
        }
        println!("sd clk divisor: 0x{:08x}, shift: 0x{:08x}", d, s);

        let mut h = 0;
        if self.host_spec_ver > HOST_SPEC_V2 {
            h = (d & 0x300) >> 2;
        }

        d = ((d & 0xff) << 8) | h;
        reg_val = regs.control1.read() & 0xffff_003f;
        regs.control1.write(reg_val | d);
        wait_msec(10);
        reg_val = regs.control1.read();
        regs.control1.write(reg_val | C1_CLK_EN);
        wait_msec(10);

        reg_val = regs.control1.read() & C1_CLK_STABLE;
        cnt = 10000;
        while cnt > 0 && reg_val == 0 {
            wait_msec(10);
            cnt -= 1;
            reg_val = regs.control1.read() & C1_CLK_STABLE;
        }

        if cnt <= 0 {
            println!("ERROR: failed to get stable clock");
            return Err(Error::new(EINVAL));
        }

        Ok(())
    }

    pub unsafe fn sd_cmd(&mut self, mut code: u32, arg: u32) -> Result<u32> {
        if (code & CMD_NEED_APP) != 0 {
            let pre_cmd = CMD_APP_CMD | if self.rca != 0 { CMD_RSPNS_48 } else { 0 };
            match self.sd_cmd(pre_cmd, self.rca) {
                Err(_) => {
                    println!("ERROR: failed to send SD APP command");
                    return Err(Error::new(EINVAL));
                }
                Ok(_) => {
                    code &= !CMD_NEED_APP;
                }
            }
        }

        if let Err(_) = self.sd_status(SR_CMD_INHIBIT) {
            println!("ERROR: Emmc busy");
            return Err(Error::new(EINVAL));
        }

        //println!("EMMC: Sending command 0x{:08x}, arg 0x{:08x}", code, arg);

        let regs = self.regs.get_mut().unwrap();
        let mut reg_val = regs.interrupt.read();
        regs.interrupt.write(reg_val);
        regs.arg1.write(arg);
        regs.cmdtm.write(code);

        if code == CMD_SEND_OP_COND {
            wait_msec(1000);
        } else if code == CMD_SEND_IF_COND || code == CMD_APP_CMD {
            wait_msec(200);
        }

        if let Err(_) = self.sd_int(INT_CMD_DONE) {
            println!("ERROR: failed to send EMMC command");
            return Err(Error::new(EINVAL));
        }

        let regs = self.regs.get_mut().unwrap();
        reg_val = regs.resp0.read();

        if code == CMD_GO_IDLE || code == CMD_APP_CMD {
            return Ok(0);
        } else if code == (CMD_APP_CMD | CMD_RSPNS_48) {
            return Ok(reg_val & SR_APP_CMD);
        } else if code == CMD_SEND_OP_COND {
            return Ok(reg_val);
        } else if code == CMD_SEND_IF_COND {
            if reg_val == arg {
                return Ok(0);
            } else {
                return Err(Error::new(EINVAL));
            }
        } else if code == CMD_ALL_SEND_CID {
            self.cid[0] = reg_val;
            self.cid[1] = regs.resp1.read();
            self.cid[2] = regs.resp2.read();
            self.cid[3] = regs.resp3.read();

            //FIXME: wrong implement, see CMD_SEND_CSD for detail
            return Ok(reg_val);
        } else if code == CMD_SEND_CSD {
            let tmp0 = reg_val;
            let tmp1 = regs.resp1.read();
            let tmp2 = regs.resp2.read();
            let tmp3 = regs.resp3.read();

            self.csd[0] = tmp3 << 8 | tmp2 >> 24;
            self.csd[1] = tmp2 << 8 | tmp1 >> 24;
            self.csd[2] = tmp1 << 8 | tmp0 >> 24;
            self.csd[3] = tmp0 << 8;

            //FIXME: support variable length of result.
            return Ok(reg_val);
        } else if code == CMD_SEND_REL_ADDR {
            let mut err = reg_val & 0x1fff;
            err |= (reg_val & 0x2000) << 6;
            err |= (reg_val & 0x4000) << 8;
            err |= (reg_val & 0x8000) << 8;
            err &= CMD_ERRORS_MASK;

            if err != 0 {
                return Err(Error::new(EINVAL));
            } else {
                return Ok(reg_val & CMD_RCA_MASK);
            }
        } else {
            return Ok(reg_val & CMD_ERRORS_MASK);
        }
    }

    pub unsafe fn sd_status(&mut self, mask: u32) -> Result<()> {
        let regs = self.regs.get_mut().unwrap();
        let mut cnt = 500000;

        let mut reg_val = regs.status.read() & mask;
        let mut reg_val1 = regs.interrupt.read() & INT_ERROR_MASK;

        while cnt > 0 && reg_val != 0 && reg_val1 == 0 {
            wait_msec(1);
            cnt -= 1;
            reg_val = regs.status.read() & mask;
            reg_val1 = regs.interrupt.read() & INT_ERROR_MASK;
        }
        reg_val1 = regs.interrupt.read() & INT_ERROR_MASK;

        if cnt <= 0 || reg_val1 != 0 {
            return Err(Error::new(EINVAL));
        } else {
            return Ok(());
        }
    }
    pub unsafe fn sd_int(&mut self, mask: u32) -> Result<()> {
        let regs = self.regs.get_mut().unwrap();
        let mut cnt = 100_0000;
        let m = mask | INT_ERROR_MASK;

        let mut reg_val = regs.interrupt.read() & m;

        while cnt > 0 && reg_val == 0 {
            wait_msec(1);
            cnt -= 1;
            reg_val = regs.interrupt.read() & m;
        }
        reg_val = regs.interrupt.read();
        let err = reg_val & (INT_CMD_TIMEOUT | INT_DATA_TIMEOUT | INT_ERROR_MASK);

        if cnt <= 0 || err != 0 {
            regs.interrupt.write(reg_val);
            return Err(Error::new(EINVAL));
        } else {
            regs.interrupt.write(mask);
            return Ok(());
        }
    }

    pub unsafe fn sd_readblock(&mut self, lba: u32, buf: &mut [u32], num: u32) -> Result<usize> {
        let num = if num < 1 { 1 } else { num };

        //println!("sd_readblock lba 0x{:x}, num 0x{:x}", lba, num);

        if let Err(_) = self.sd_status(SR_DAT_INHIBIT) {
            println!("SR_DAT_INHIBIT TIMEOUT");
            return Err(Error::new(EINVAL));
        }

        if (self.scr[0] & SCR_SUPP_CCS) != 0 {
            if num > 1 && ((self.scr[0] & SCR_SUPP_SET_BLKCNT) != 0) {
                if let Err(_) = self.sd_cmd(CMD_SET_BLOCKCNT, num) {
                    println!("CMD_SET_BLOCKCNT ERROR");
                    return Err(Error::new(EINVAL));
                }
            }
            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write((num) << 16 | 512);
            if num == 1 {
                self.sd_cmd(CMD_READ_SINGLE, lba).unwrap();
            } else {
                self.sd_cmd(CMD_READ_MULTI, lba).unwrap();
            }
        } else {
            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write(1 << 16 | 512);
        }

        let mut cnt = 0;
        while cnt < num {
            if (self.scr[0] & SCR_SUPP_CCS) == 0 {
                self.sd_cmd(CMD_READ_SINGLE, (lba + cnt) * 512).unwrap();
            }

            if let Err(_) = self.sd_int(INT_READ_RDY) {
                println!("ERROR: Timeout waiting for ready to read");
                return Err(Error::new(EINVAL));
            }

            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write(1 << 16 | 512);
            for d in 0..128 {
                buf[(128 * cnt + d) as usize] = regs.data.read();
            }
            cnt += 1;
        }

        if num > 1 && (self.scr[0] & SCR_SUPP_SET_BLKCNT) == 0 && (self.scr[0] & SCR_SUPP_CCS) != 0
        {
            self.sd_cmd(CMD_STOP_TRANS, 0).unwrap();
        }
        Ok((num * 512) as usize)
    }

    pub unsafe fn sd_writeblock(&mut self, lba: u32, buf: &[u32], num: u32) -> Result<usize> {
        let num = if num < 1 { 1 } else { num };

        //println!("sd_writelock lba 0x{:x}, num 0x{:x}", lba, num);

        if let Err(_) = self.sd_status(SR_DAT_INHIBIT | SR_WRITE_AVAILABLE) {
            println!("SR_DAT_INHIBIT TIMEOUT");
            return Err(Error::new(EINVAL));
        }

        if (self.scr[0] & SCR_SUPP_CCS) != 0 {
            if num > 1 && ((self.scr[0] & SCR_SUPP_SET_BLKCNT) != 0) {
                if let Err(_) = self.sd_cmd(CMD_SET_BLOCKCNT, num) {
                    println!("CMD_SET_BLOCKCNT ERROR");
                    return Err(Error::new(EINVAL));
                }
            }
            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write((num) << 16 | 512);
            if num == 1 {
                self.sd_cmd(CMD_WRITE_SINGLE, lba).unwrap();
            } else {
                self.sd_cmd(CMD_WRITE_MULTI, lba).unwrap();
            }
        } else {
            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write(1 << 16 | 512);
        }

        let mut cnt = 0;
        while cnt < num {
            if (self.scr[0] & SCR_SUPP_CCS) == 0 {
                self.sd_cmd(CMD_WRITE_SINGLE, (lba + cnt) * 512).unwrap();
            }

            if let Err(_) = self.sd_int(INT_WRITE_RDY) {
                println!("ERROR: Timeout waiting for ready to write");
                return Err(Error::new(EINVAL));
            }

            let regs = self.regs.get_mut().unwrap();
            regs.blksizecnt.write(1 << 16 | 512);
            for d in 0..128 {
                regs.data.write(buf[(128 * cnt + d) as usize]);
            }
            cnt += 1;
        }

        if let Err(_) = self.sd_int(INT_DATA_DONE) {
            println!("ERROR: Timeout waiting for data done");
            return Err(Error::new(EINVAL));
        }

        if num > 1 && (self.scr[0] & SCR_SUPP_SET_BLKCNT) == 0 && (self.scr[0] & SCR_SUPP_CCS) != 0
        {
            self.sd_cmd(CMD_STOP_TRANS, 0).unwrap();
        }
        Ok((num * 512) as usize)
    }
}

impl Disk for SdHostCtrl {
    fn block_size(&self) -> u32 {
        512
    }

    fn size(&self) -> u64 {
        //assert 512MiB
        self.size
    }

    // TODO: real async?
    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize> {
        if (buffer.len() % 512) != 0 {
            println!("buffer.len {} should be aligned to {}", buffer.len(), 512);
            return Err(Error::new(EINVAL));
        }
        let u32_len = buffer.len() / core::mem::size_of::<u32>();
        let num = buffer.len() / 512;
        let u8_ptr = buffer.as_mut_ptr();
        let ret = unsafe {
            let u32_buffer = core::slice::from_raw_parts_mut(u8_ptr as *mut u32, u32_len);
            self.sd_readblock(block as u32, u32_buffer, num as u32)
        };
        match ret {
            Ok(cnt) => Ok(cnt),
            Err(err) => Err(err),
        }
    }

    // TODO: real async?
    async fn write(&mut self, block: u64, buffer: &[u8]) -> Result<usize> {
        if (buffer.len() % 512) != 0 {
            println!("buffer.len {} should be aligned to {}", buffer.len(), 512);
            return Err(Error::new(EINVAL));
        }
        let u32_len = buffer.len() / core::mem::size_of::<u32>();
        let num = buffer.len() / 512;
        let u8_ptr = buffer.as_ptr();
        let ret = unsafe {
            let u32_buffer = core::slice::from_raw_parts(u8_ptr as *const u32, u32_len);
            self.sd_writeblock(block as u32, u32_buffer, num as u32)
        };
        match ret {
            Ok(cnt) => Ok(cnt),
            Err(err) => Err(err),
        }
    }
}
