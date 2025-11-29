use std::convert::TryInto;
use std::{ptr, thread, time};

use common::io::{Io, Mmio};
use redox_scheme::scheme::SchemeSync;
use redox_scheme::CallerCtx;
use redox_scheme::OpenResult;
use syscall::error::{Error, Result, EACCES, EINVAL, EIO, EWOULDBLOCK};
use syscall::flag::{EventFlags, O_NONBLOCK};
use syscall::schemev2::NewFdFlags;

use common::dma::Dma;

use self::regs::*;

mod regs;

const ERR_ALOAD: usize = 1;
const ERR_RSTMAC: usize = 2;
const ERR_PARM: usize = 3;
const ERR_MIIBUSY: usize = 4;
const LINK_TIMEOUT: usize = 8;

const FLAG_HALT: u32 = 0;
const FLAG_TASK_RESET: u32 = 1;
const FLAG_TASK_CHK_LINK: u32 = 2;
const FLAG_TASK_UPDATE_SMB: u32 = 3;

const HALF_DUPLEX: u8 = 1;
const FULL_DUPLEX: u8 = 2;

const SPEED_0: u16 = 0;
const SPEED_10: u16 = 10;
const SPEED_100: u16 = 100;
const SPEED_1000: u16 = 1000;

const FC_RX: u8 = 0x01;
const FC_TX: u8 = 0x02;
const FC_ANEG: u8 = 0x04;

const CAP_GIGA: u32 = 1 << 0;
const CAP_PTP: u32 = 1 << 1;
const CAP_AZ: u32 = 1 << 2;
const CAP_L0S: u32 = 1 << 3;
const CAP_L1: u32 = 1 << 4;
const CAP_SWOI: u32 = 1 << 5;
const CAP_RSS: u32 = 1 << 6;
const CAP_MSIX: u32 = 1 << 7;
/* support Multi-TX-Q */
const CAP_MTQ: u32 = 1 << 8;
/* support Multi-RX-Q */
const CAP_MRQ: u32 = 1 << 9;

const ISR_MISC: u32 = ISR_PCIE_LNKDOWN | ISR_DMAW | ISR_DMAR | ISR_SMB | ISR_MANU | ISR_TIMER;

const ISR_FATAL: u32 = ISR_PCIE_LNKDOWN | ISR_DMAW | ISR_DMAR;

const ISR_ALERT: u32 = ISR_RXF_OV | ISR_TXF_UR | ISR_RFD_UR;

const ISR_ALL_QUEUES: u32 = ISR_TX_Q0
    | ISR_TX_Q1
    | ISR_TX_Q2
    | ISR_TX_Q3
    | ISR_RX_Q0
    | ISR_RX_Q1
    | ISR_RX_Q2
    | ISR_RX_Q3
    | ISR_RX_Q4
    | ISR_RX_Q5
    | ISR_RX_Q6
    | ISR_RX_Q7;

const PCI_COMMAND_IO: u16 = 0x1; /* Enable response in I/O space */
const PCI_COMMAND_MEMORY: u16 = 0x2; /* Enable response in Memory space */
const PCI_COMMAND_MASTER: u16 = 0x4; /* Enable bus mastering */
const PCI_COMMAND_SPECIAL: u16 = 0x8; /* Enable response to special cycles */
const PCI_COMMAND_INVALIDATE: u16 = 0x10; /* Use memory write and invalidate */
const PCI_COMMAND_VGA_PALETTE: u16 = 0x20; /* Enable palette snooping */
const PCI_COMMAND_PARITY: u16 = 0x40; /* Enable parity checking */
const PCI_COMMAND_WAIT: u16 = 0x80; /* Enable address/data stepping */
const PCI_COMMAND_SERR: u16 = 0x100; /* Enable SERR */
const PCI_COMMAND_FAST_BACK: u16 = 0x200; /* Enable back-to-back writes */
const PCI_COMMAND_INTX_DISABLE: u16 = 0x400; /* INTx Emulation Disable */

/// MII basic mode control register
const MII_BMCR: u16 = 0x00;
const BMCR_FULLDPLX: u16 = 0x0100;
const BMCR_ANRESTART: u16 = 0x0200;
const BMCR_ANENABLE: u16 = 0x1000;
const BMCR_SPEED100: u16 = 0x2000;
const BMCR_RESET: u16 = 0x8000;

/// MII basic mode status register
const MII_BMSR: u16 = 0x01;
const BMSR_LSTATUS: u16 = 0x0004;

/// MII advertisement register
const MII_ADVERTISE: u16 = 0x04;

/// MII 1000BASE-T control
const MII_CTRL1000: u16 = 0x09;

const ETH_HLEN: u16 = 14;

const ADVERTISED_10baseT_Half: u32 = 1 << 0;
const ADVERTISED_10baseT_Full: u32 = 1 << 1;
const ADVERTISED_100baseT_Half: u32 = 1 << 2;
const ADVERTISED_100baseT_Full: u32 = 1 << 3;
const ADVERTISED_1000baseT_Half: u32 = 1 << 4;
const ADVERTISED_1000baseT_Full: u32 = 1 << 5;
const ADVERTISED_Autoneg: u32 = 1 << 6;
const ADVERTISED_Pause: u32 = 1 << 13;
const ADVERTISED_Asym_Pause: u32 = 1 << 14;

const ADVERTISE_CSMA: u32 = 0x0001; /* Only selector supported     */
const ADVERTISE_10HALF: u32 = 0x0020; /* Try for 10mbps half-duplex  */
const ADVERTISE_1000XFULL: u32 = 0x0020; /* Try for 1000BASE-X full-duplex */
const ADVERTISE_10FULL: u32 = 0x0040; /* Try for 10mbps full-duplex  */
const ADVERTISE_1000XHALF: u32 = 0x0040; /* Try for 1000BASE-X half-duplex */
const ADVERTISE_100HALF: u32 = 0x0080; /* Try for 100mbps half-duplex */
const ADVERTISE_1000XPAUSE: u32 = 0x0080; /* Try for 1000BASE-X pause    */
const ADVERTISE_100FULL: u32 = 0x0100; /* Try for 100mbps full-duplex */
const ADVERTISE_1000XPSE_ASYM: u32 = 0x0100; /* Try for 1000BASE-X asym pause */
const ADVERTISE_100BASE4: u32 = 0x0200; /* Try for 100mbps 4k packets  */
const ADVERTISE_PAUSE_CAP: u32 = 0x0400; /* Try for pause               */
const ADVERTISE_PAUSE_ASYM: u32 = 0x0800; /* Try for asymetric pause     */

const ADVERTISE_1000HALF: u32 = 0x0100;
const ADVERTISE_1000FULL: u32 = 0x0200;

macro_rules! FIELD_GETX {
    ($x:expr, $name:ident) => {
        ((($x) >> ${concat($name, _SHIFT)} & ${concat($name, _MASK)}))
    };
}

macro_rules! FIELDX {
    ($name:ident, $v:expr) => {
        (((($v) as u32) & ${concat($name, _MASK)}) << ${concat($name, _SHIFT)})
    };
}

macro_rules! FIELD_SETS {
    ($x:expr, $name:ident, $v:expr) => {{
        ($x) = (($x) & !(${concat($name, _MASK)} << ${concat($name, _SHIFT)}))
            | (((($v) as u16) & ${concat($name, _MASK)}) << ${concat($name, _SHIFT)})
    }};
}

macro_rules! FIELD_SET32 {
    ($x:expr, $name:ident, $v:expr) => {{
        ($x) = (($x) & !(${concat($name, _MASK)} << ${concat($name, _SHIFT)}))
            | (((($v) as u32) & ${concat($name, _MASK)}) << ${concat($name, _SHIFT)})
    }};
}

fn udelay(micros: u32) {
    thread::sleep(time::Duration::new(0, micros * 1000));
}

fn ethtool_adv_to_mii_adv_t(ethadv: u32) -> u32 {
    let mut result: u32 = 0;

    if (ethadv & ADVERTISED_10baseT_Half > 0) {
        result |= ADVERTISE_10HALF;
    }
    if (ethadv & ADVERTISED_10baseT_Full > 0) {
        result |= ADVERTISE_10FULL;
    }
    if (ethadv & ADVERTISED_100baseT_Half > 0) {
        result |= ADVERTISE_100HALF;
    }
    if (ethadv & ADVERTISED_100baseT_Full > 0) {
        result |= ADVERTISE_100FULL;
    }
    if (ethadv & ADVERTISED_Pause > 0) {
        result |= ADVERTISE_PAUSE_CAP;
    }
    if (ethadv & ADVERTISED_Asym_Pause > 0) {
        result |= ADVERTISE_PAUSE_ASYM;
    }

    return result;
}

fn ethtool_adv_to_mii_ctrl1000_t(ethadv: u32) -> u32 {
    let mut result: u32 = 0;

    if (ethadv & ADVERTISED_1000baseT_Half > 0) {
        result |= ADVERTISE_1000HALF;
    }
    if (ethadv & ADVERTISED_1000baseT_Full > 0) {
        result |= ADVERTISE_1000FULL;
    }

    return result;
}

/// Transmit packet descriptor
#[repr(C, packed)]
struct Tpd {
    blen: Mmio<u16>,
    vlan: Mmio<u16>,
    flags: Mmio<u32>,
    addr_low: Mmio<u32>,
    addr_high: Mmio<u32>,
}

/// Receive free descriptor
#[repr(C, packed)]
struct Rfd {
    addr_low: Mmio<u32>,
    addr_high: Mmio<u32>,
}

/// Receive return descriptor
#[repr(C, packed)]
struct Rrd {
    checksum: Mmio<u16>,
    rfd: Mmio<u16>,
    rss: Mmio<u32>,
    vlan: Mmio<u16>,
    proto: Mmio<u8>,
    rss_flags: Mmio<u8>,
    len: Mmio<u16>,
    flags: Mmio<u16>,
}

pub struct Alx {
    base: usize,

    vendor_id: u16,
    device_id: u16,
    subdev_id: u16,
    subven_id: u16,
    revision: u8,

    cap: u32,
    flag: u32,

    mtu: u16,
    imt: u16,
    dma_chnl: u8,
    ith_tpd: u32,
    mc_hash: [u32; 2],

    wrr: [u32; 4],
    wrr_ctrl: u32,

    imask: u32,
    smb_timer: u32,
    link_up: bool,
    link_speed: u16,
    link_duplex: u8,

    adv_cfg: u32,
    flowctrl: u8,

    rx_ctrl: u32,

    lnk_patch: bool,
    hib_patch: bool,
    is_fpga: bool,

    rfd_buffer: [Dma<[u8; 16384]>; 16],
    rfd_ring: Dma<[Rfd; 16]>,
    rrd_ring: Dma<[Rrd; 16]>,
    tpd_buffer: [Dma<[u8; 16384]>; 16],
    tpd_ring: [Dma<[Tpd; 16]>; 4],
}

fn dma_array<T, const N: usize>() -> Result<[Dma<T>; N]> {
    Ok((0..N)
        .map(|_| Ok(Dma::zeroed().map(|dma| unsafe { dma.assume_init() })?))
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .unwrap_or_else(|_| unreachable!()))
}

impl Alx {
    pub unsafe fn new(base: usize) -> Result<Self> {
        let mut module = Alx {
            base,

            vendor_id: 0,
            device_id: 0,
            subdev_id: 0,
            subven_id: 0,
            revision: 0,

            cap: 0,
            flag: 0,

            mtu: 1500, /*TODO: Get from adapter?*/
            imt: 200,
            dma_chnl: 0,
            ith_tpd: 5, /* ~ size of tpd_ring / 3 */
            mc_hash: [0; 2],

            wrr: [4; 4],
            wrr_ctrl: WRR_PRI_RESTRICT_NONE,

            imask: ISR_MISC,
            smb_timer: 400,
            link_up: false,
            link_speed: 0,
            link_duplex: 0,

            adv_cfg: ADVERTISED_Autoneg
                | ADVERTISED_10baseT_Half
                | ADVERTISED_10baseT_Full
                | ADVERTISED_100baseT_Full
                | ADVERTISED_100baseT_Half
                | ADVERTISED_1000baseT_Full,
            flowctrl: FC_ANEG | FC_RX | FC_TX,

            rx_ctrl: MAC_CTRL_WOLSPED_SWEN
                | MAC_CTRL_MHASH_ALG_HI5B
                | MAC_CTRL_BRD_EN
                | MAC_CTRL_PCRCE
                | MAC_CTRL_CRCE
                | MAC_CTRL_RXFC_EN
                | MAC_CTRL_TXFC_EN
                | FIELDX!(MAC_CTRL_PRMBLEN, 7),

            lnk_patch: false,
            hib_patch: false,
            is_fpga: false,

            rfd_buffer: dma_array()?,
            rfd_ring: Dma::zeroed()?.assume_init(),
            rrd_ring: Dma::zeroed()?.assume_init(),
            tpd_buffer: dma_array()?,
            tpd_ring: dma_array()?,
        };

        module.init()?;

        Ok(module)
    }

    pub fn revid(&self) -> u8 {
        self.revision >> PCI_REVID_SHIFT
    }

    pub fn with_cr(&self) -> bool {
        self.revision & 1 > 0
    }

    unsafe fn handle_intr_misc(&mut self, intr: u32) -> bool {
        if (intr & ISR_FATAL > 0) {
            println!("intr-fatal: {:X}", intr);
            self.flag |= FLAG_TASK_RESET;
            self.task();
            return true;
        }

        if (intr & ISR_ALERT > 0) {
            println!("interrupt alert: {:X}", intr);
        }

        if (intr & ISR_SMB > 0) {
            self.flag |= FLAG_TASK_UPDATE_SMB;
            self.task();
        }

        if (intr & ISR_PHY > 0) {
            /* suppress PHY interrupt, because the source
             * is from PHY internal. only the internal status
             * is cleared, the interrupt status could be cleared.
             */
            self.imask &= !ISR_PHY;
            let imask = self.imask;
            self.reg_write(IMR, imask);
            self.flag |= FLAG_TASK_CHK_LINK;
            self.task();
        }

        return false;
    }

    unsafe fn intr_1(&mut self, mut intr: u32) -> bool {
        /* ACK interrupt */
        println!("ACK interrupt: {:X}", intr | ISR_DIS);
        self.reg_write(ISR, intr | ISR_DIS);
        intr &= self.imask;

        if (self.handle_intr_misc(intr)) {
            return true;
        }

        if (intr & (ISR_TX_Q0 | ISR_RX_Q0) > 0) {
            println!("TX | RX");
            //TODO: napi_schedule(&adpt->qnapi[0]->napi);
            /* mask rx/tx interrupt, enable them when napi complete */
            self.imask &= !ISR_ALL_QUEUES;
            let imask = self.imask;
            self.reg_write(IMR, imask);
        }

        self.reg_write(ISR, 0);

        return true;
    }

    pub unsafe fn intr_legacy(&mut self) -> bool {
        /* read interrupt status */
        let intr = self.reg_read(ISR);
        if (intr & ISR_DIS > 0 || intr & self.imask == 0) {
            let mask = self.reg_read(IMR);
            println!(
                "seems a wild interrupt, intr={:X}, imask={:X}, mask={:X}",
                intr, self.imask, mask
            );

            return false;
        }

        return self.intr_1(intr);
    }

    pub fn next_reg_read(&self) -> usize {
        /*
        let head = unsafe { self.reg_read(RDH) };
        let mut tail = unsafe { self.reg_read(RDT) };

        tail += 1;
        if tail >= self.receive_ring.len() as u32 {
            tail = 0;
        }

        if tail != head {
            let rd = unsafe { &* (self.receive_ring.as_ptr().offset(tail as isize) as *const Rd) };
            if rd.status & RD_DD == RD_DD {
                return rd.length as usize;
            }
        }

        0
        */
        0
    }

    unsafe fn reg_read(&self, register: u32) -> u32 {
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    unsafe fn reg_write(&self, register: u32, data: u32) -> u32 {
        ptr::write_volatile((self.base + register as usize) as *mut u32, data);
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    unsafe fn wait_mdio_idle(&mut self) -> bool {
        let mut val: u32;
        let mut i: u32 = 0;

        while (i < MDIO_MAX_AC_TO) {
            val = self.reg_read(MDIO);
            if (val & MDIO_BUSY == 0) {
                break;
            }
            udelay(10);
            i += 1;
        }
        return i != MDIO_MAX_AC_TO;
    }

    unsafe fn stop_phy_polling(&mut self) {
        if (!self.is_fpga) {
            return;
        }

        self.reg_write(MDIO, 0);
        self.wait_mdio_idle();
    }

    unsafe fn start_phy_polling(&mut self, clk_sel: u16) {
        let mut val: u32;

        if (!self.is_fpga) {
            return;
        }

        val = MDIO_SPRES_PRMBL
            | FIELDX!(MDIO_CLK_SEL, clk_sel)
            | FIELDX!(MDIO_REG, 1)
            | MDIO_START
            | MDIO_OP_READ;
        self.reg_write(MDIO, val);
        self.wait_mdio_idle();
        val |= MDIO_AUTO_POLLING;
        val &= !MDIO_START;
        self.reg_write(MDIO, val);
        udelay(30);
    }

    unsafe fn read_phy_core(&mut self, ext: bool, dev: u8, reg: u16, phy_data: &mut u16) -> usize {
        let mut val: u32;
        let clk_sel: u16;
        let err: usize;

        self.stop_phy_polling();

        *phy_data = 0;

        /* use slow clock when it's in hibernation status */
        clk_sel = if !self.link_up {
            MDIO_CLK_SEL_25MD128
        } else {
            MDIO_CLK_SEL_25MD4
        };

        if (ext) {
            val = FIELDX!(MDIO_EXTN_DEVAD, dev) | FIELDX!(MDIO_EXTN_REG, reg);
            self.reg_write(MDIO_EXTN, val);

            val = MDIO_SPRES_PRMBL
                | FIELDX!(MDIO_CLK_SEL, clk_sel)
                | MDIO_START
                | MDIO_MODE_EXT
                | MDIO_OP_READ;
        } else {
            val = MDIO_SPRES_PRMBL
                | FIELDX!(MDIO_CLK_SEL, clk_sel)
                | FIELDX!(MDIO_REG, reg)
                | MDIO_START
                | MDIO_OP_READ;
        }
        self.reg_write(MDIO, val);

        if (!self.wait_mdio_idle()) {
            err = ERR_MIIBUSY;
        } else {
            val = self.reg_read(MDIO);
            *phy_data = FIELD_GETX!(val, MDIO_DATA) as u16;
            err = 0;
        }

        self.start_phy_polling(clk_sel);

        return err;
    }

    unsafe fn write_phy_core(&mut self, ext: bool, dev: u8, reg: u16, phy_data: u16) -> usize {
        let mut val: u32;
        let clk_sel: u16;
        let mut err: usize = 0;

        self.stop_phy_polling();

        /* use slow clock when it's in hibernation status */
        clk_sel = if !self.link_up {
            MDIO_CLK_SEL_25MD128
        } else {
            MDIO_CLK_SEL_25MD4
        };

        if (ext) {
            val = FIELDX!(MDIO_EXTN_DEVAD, dev) | FIELDX!(MDIO_EXTN_REG, reg);
            self.reg_write(MDIO_EXTN, val);

            val = MDIO_SPRES_PRMBL
                | FIELDX!(MDIO_CLK_SEL, clk_sel)
                | FIELDX!(MDIO_DATA, phy_data)
                | MDIO_START
                | MDIO_MODE_EXT;
        } else {
            val = MDIO_SPRES_PRMBL
                | FIELDX!(MDIO_CLK_SEL, clk_sel)
                | FIELDX!(MDIO_REG, reg)
                | FIELDX!(MDIO_DATA, phy_data)
                | MDIO_START;
        }
        self.reg_write(MDIO, val);

        if !self.wait_mdio_idle() {
            err = ERR_MIIBUSY;
        }

        self.start_phy_polling(clk_sel);

        return err;
    }

    unsafe fn read_phy_reg(&mut self, reg: u16, phy_data: &mut u16) -> usize {
        self.read_phy_core(false, 0, reg, phy_data)
    }

    unsafe fn write_phy_reg(&mut self, reg: u16, phy_data: u16) -> usize {
        self.write_phy_core(false, 0, reg, phy_data)
    }

    unsafe fn read_phy_ext(&mut self, dev: u8, reg: u16, data: &mut u16) -> usize {
        self.read_phy_core(true, dev, reg, data)
    }

    unsafe fn write_phy_ext(&mut self, dev: u8, reg: u16, data: u16) -> usize {
        self.write_phy_core(true, dev, reg, data)
    }

    unsafe fn read_phy_dbg(&mut self, reg: u16, data: &mut u16) -> usize {
        let err = self.write_phy_reg(MII_DBG_ADDR, reg);
        if (err > 0) {
            return err;
        }

        self.read_phy_reg(MII_DBG_DATA, data)
    }

    unsafe fn write_phy_dbg(&mut self, reg: u16, data: u16) -> usize {
        let err = self.write_phy_reg(MII_DBG_ADDR, reg);
        if (err > 0) {
            return err;
        }

        self.write_phy_reg(MII_DBG_DATA, data)
    }

    unsafe fn enable_aspm(&mut self, l0s_en: bool, l1_en: bool) {
        let mut pmctrl: u32;
        let rev: u8 = self.revid();

        pmctrl = self.reg_read(PMCTRL);

        FIELD_SET32!(pmctrl, PMCTRL_LCKDET_TIMER, PMCTRL_LCKDET_TIMER_DEF);
        pmctrl |= PMCTRL_RCVR_WT_1US | PMCTRL_L1_CLKSW_EN | PMCTRL_L1_SRDSRX_PWD;
        FIELD_SET32!(pmctrl, PMCTRL_L1REQ_TO, PMCTRL_L1REG_TO_DEF);
        FIELD_SET32!(pmctrl, PMCTRL_L1_TIMER, PMCTRL_L1_TIMER_16US);
        pmctrl &= !(PMCTRL_L1_SRDS_EN
            | PMCTRL_L1_SRDSPLL_EN
            | PMCTRL_L1_BUFSRX_EN
            | PMCTRL_SADLY_EN
            | PMCTRL_HOTRST_WTEN
            | PMCTRL_L0S_EN
            | PMCTRL_L1_EN
            | PMCTRL_ASPM_FCEN
            | PMCTRL_TXL1_AFTER_L0S
            | PMCTRL_RXL1_AFTER_L0S);
        if ((rev == REV_A0 || rev == REV_A1) && self.with_cr()) {
            pmctrl |= PMCTRL_L1_SRDS_EN | PMCTRL_L1_SRDSPLL_EN;
        }

        if (l0s_en) {
            pmctrl |= (PMCTRL_L0S_EN | PMCTRL_ASPM_FCEN);
        }
        if (l1_en) {
            pmctrl |= (PMCTRL_L1_EN | PMCTRL_ASPM_FCEN);
        }

        self.reg_write(PMCTRL, pmctrl);
    }

    unsafe fn reset_pcie(&mut self) {
        let mut val: u32;
        let rev: u8 = self.revid();

        /* Workaround for PCI problem when BIOS sets MMRBC incorrectly. */
        let mut val16 = ptr::read((self.base + 4) as *const u16);
        if (val16 & (PCI_COMMAND_MASTER | PCI_COMMAND_MEMORY | PCI_COMMAND_IO) == 0
            || val16 & PCI_COMMAND_INTX_DISABLE > 0)
        {
            println!("Fix PCI_COMMAND_INTX_DISABLE");
            val16 = (val16 | (PCI_COMMAND_MASTER | PCI_COMMAND_MEMORY | PCI_COMMAND_IO))
                & !PCI_COMMAND_INTX_DISABLE;
            ptr::write((self.base + 4) as *mut u16, val16);
        }

        /* clear WoL setting/status */
        self.reg_read(WOL0);
        self.reg_write(WOL0, 0);

        /* deflt val of PDLL D3PLLOFF */
        val = self.reg_read(PDLL_TRNS1);
        self.reg_write(PDLL_TRNS1, val & !PDLL_TRNS1_D3PLLOFF_EN);

        /* mask some pcie error bits */
        val = self.reg_read(UE_SVRT);
        val &= !(UE_SVRT_DLPROTERR | UE_SVRT_FCPROTERR);
        self.reg_write(UE_SVRT, val);

        /* wol 25M  & pclk */
        val = self.reg_read(MASTER);
        if ((rev == REV_A0 || rev == REV_A1) && self.with_cr()) {
            if ((val & MASTER_WAKEN_25M) == 0 || (val & MASTER_PCLKSEL_SRDS) == 0) {
                self.reg_write(MASTER, val | MASTER_PCLKSEL_SRDS | MASTER_WAKEN_25M);
            }
        } else {
            if ((val & MASTER_WAKEN_25M) == 0 || (val & MASTER_PCLKSEL_SRDS) != 0) {
                self.reg_write(MASTER, (val & !MASTER_PCLKSEL_SRDS) | MASTER_WAKEN_25M);
            }
        }

        /* ASPM setting */
        let l0s_en = self.cap & CAP_L0S > 0;
        let l1_en = self.cap & CAP_L1 > 0;
        self.enable_aspm(l0s_en, l1_en);

        udelay(10);
    }

    unsafe fn reset_phy(&mut self) {
        let mut i: u32;
        let mut val: u32;
        let mut phy_val: u16 = 0;

        /* (DSP)reset PHY core */
        val = self.reg_read(PHY_CTRL);
        val &= !(PHY_CTRL_DSPRST_OUT
            | PHY_CTRL_IDDQ
            | PHY_CTRL_GATE_25M
            | PHY_CTRL_POWER_DOWN
            | PHY_CTRL_CLS);
        val |= PHY_CTRL_RST_ANALOG;

        if (!self.hib_patch) {
            val |= (PHY_CTRL_HIB_PULSE | PHY_CTRL_HIB_EN);
        } else {
            val &= !(PHY_CTRL_HIB_PULSE | PHY_CTRL_HIB_EN);
        }
        self.reg_write(PHY_CTRL, val);
        udelay(10);
        self.reg_write(PHY_CTRL, val | PHY_CTRL_DSPRST_OUT);

        /* delay 800us */
        i = 0;
        while (i < PHY_CTRL_DSPRST_TO) {
            udelay(10);
            i += 1;
        }

        if !self.is_fpga {
            /* phy power saving & hib */
            if (!self.hib_patch) {
                self.write_phy_dbg(MIIDBG_LEGCYPS, LEGCYPS_DEF);
                self.write_phy_dbg(MIIDBG_SYSMODCTRL, SYSMODCTRL_IECHOADJ_DEF);
                self.write_phy_ext(MIIEXT_PCS, MIIEXT_VDRVBIAS, VDRVBIAS_DEF);
            } else {
                self.write_phy_dbg(MIIDBG_LEGCYPS, LEGCYPS_DEF & !LEGCYPS_EN);
                self.write_phy_dbg(MIIDBG_HIBNEG, HIBNEG_NOHIB);
                self.write_phy_dbg(MIIDBG_GREENCFG, GREENCFG_DEF);
            }

            /* EEE advertisement */
            if (self.cap & CAP_AZ > 0) {
                let eeeadv = if self.cap & CAP_GIGA > 0 {
                    LOCAL_EEEADV_1000BT | LOCAL_EEEADV_100BT
                } else {
                    LOCAL_EEEADV_100BT
                };
                self.write_phy_ext(MIIEXT_ANEG, MIIEXT_LOCAL_EEEADV, eeeadv);
                /* half amplify */
                self.write_phy_dbg(MIIDBG_AZ_ANADECT, AZ_ANADECT_DEF);
            } else {
                val = self.reg_read(LPI_CTRL);
                self.reg_write(LPI_CTRL, val & (!LPI_CTRL_EN));
                self.write_phy_ext(MIIEXT_ANEG, MIIEXT_LOCAL_EEEADV, 0);
            }

            /* phy power saving */
            self.write_phy_dbg(MIIDBG_TST10BTCFG, TST10BTCFG_DEF);
            self.write_phy_dbg(MIIDBG_SRDSYSMOD, SRDSYSMOD_DEF);
            self.write_phy_dbg(MIIDBG_TST100BTCFG, TST100BTCFG_DEF);
            self.write_phy_dbg(MIIDBG_ANACTRL, ANACTRL_DEF);
            self.read_phy_dbg(MIIDBG_GREENCFG2, &mut phy_val);
            self.write_phy_dbg(MIIDBG_GREENCFG2, phy_val & (!GREENCFG2_GATE_DFSE_EN));
            /* rtl8139c, 120m issue */
            self.write_phy_ext(MIIEXT_ANEG, MIIEXT_NLP78, MIIEXT_NLP78_120M_DEF);
            self.write_phy_ext(MIIEXT_ANEG, MIIEXT_S3DIG10, MIIEXT_S3DIG10_DEF);

            if (self.lnk_patch) {
                /* Turn off half amplitude */
                self.read_phy_ext(MIIEXT_PCS, MIIEXT_CLDCTRL3, &mut phy_val);
                self.write_phy_ext(
                    MIIEXT_PCS,
                    MIIEXT_CLDCTRL3,
                    phy_val | CLDCTRL3_BP_CABLE1TH_DET_GT,
                );
                /* Turn off Green feature */
                self.read_phy_dbg(MIIDBG_GREENCFG2, &mut phy_val);
                self.write_phy_dbg(MIIDBG_GREENCFG2, phy_val | GREENCFG2_BP_GREEN);
                /* Turn off half Bias */
                self.read_phy_ext(MIIEXT_PCS, MIIEXT_CLDCTRL5, &mut phy_val);
                self.write_phy_ext(
                    MIIEXT_PCS,
                    MIIEXT_CLDCTRL5,
                    phy_val | CLDCTRL5_BP_VD_HLFBIAS,
                );
            }
        }

        /* set phy interrupt mask */
        self.write_phy_reg(MII_IER, IER_LINK_UP | IER_LINK_DOWN);
    }

    unsafe fn stop_mac(&mut self) -> usize {
        let txq: u32;
        let rxq: u32;
        let mut val: u32;
        let mut i: u32;

        rxq = self.reg_read(RXQ0);
        self.reg_write(RXQ0, rxq & (!RXQ0_EN));
        txq = self.reg_read(TXQ0);
        self.reg_write(TXQ0, txq & (!TXQ0_EN));

        udelay(40);

        self.rx_ctrl &= !(MAC_CTRL_RX_EN | MAC_CTRL_TX_EN);
        self.reg_write(MAC_CTRL, self.rx_ctrl);

        i = 0;
        while i < DMA_MAC_RST_TO {
            val = self.reg_read(MAC_STS);
            if (val & MAC_STS_IDLE == 0) {
                break;
            }
            udelay(10);
            i += 1;
        }

        return if (DMA_MAC_RST_TO == i) {
            ERR_RSTMAC as usize
        } else {
            0
        };
    }

    unsafe fn start_mac(&mut self) {
        let mut mac: u32;
        let txq: u32;
        let rxq: u32;

        rxq = self.reg_read(RXQ0);
        self.reg_write(RXQ0, rxq | RXQ0_EN);
        txq = self.reg_read(TXQ0);
        self.reg_write(TXQ0, txq | TXQ0_EN);

        mac = self.rx_ctrl;
        if (self.link_duplex == FULL_DUPLEX) {
            mac |= MAC_CTRL_FULLD;
        } else {
            mac &= !MAC_CTRL_FULLD;
        }
        FIELD_SET32!(
            mac,
            MAC_CTRL_SPEED,
            if self.link_speed == 1000 {
                MAC_CTRL_SPEED_1000
            } else {
                MAC_CTRL_SPEED_10_100
            }
        );
        mac |= MAC_CTRL_TX_EN | MAC_CTRL_RX_EN;
        self.rx_ctrl = mac;
        self.reg_write(MAC_CTRL, mac);
    }

    unsafe fn reset_osc(&mut self, rev: u8) {
        let mut val: u32;
        let mut val2: u32;

        /* clear Internal OSC settings, switching OSC by hw itself */
        val = self.reg_read(MISC3);
        self.reg_write(MISC3, (val & !MISC3_25M_BY_SW) | MISC3_25M_NOTO_INTNL);

        /* 25M clk from chipset may be unstable 1s after de-assert of
         * PERST, driver need re-calibrate before enter Sleep for WoL
         */
        val = self.reg_read(MISC);
        if (rev >= REV_B0) {
            /* restore over current protection def-val,
             * this val could be reset by MAC-RST
             */
            FIELD_SET32!(val, MISC_PSW_OCP, MISC_PSW_OCP_DEF);
            /* a 0->1 change will update the internal val of osc */
            val &= !MISC_INTNLOSC_OPEN;
            self.reg_write(MISC, val);
            self.reg_write(MISC, val | MISC_INTNLOSC_OPEN);
            /* hw will automatically dis OSC after cab. */
            val2 = self.reg_read(MSIC2);
            val2 &= !MSIC2_CALB_START;
            self.reg_write(MSIC2, val2);
            self.reg_write(MSIC2, val2 | MSIC2_CALB_START);
        } else {
            val &= !MISC_INTNLOSC_OPEN;
            /* disable isoloate for A0 */
            if (rev == REV_A0 || rev == REV_A1) {
                val &= !MISC_ISO_EN;
            }

            self.reg_write(MISC, val | MISC_INTNLOSC_OPEN);
            self.reg_write(MISC, val);
        }

        udelay(20);
    }

    unsafe fn reset_mac(&mut self) -> usize {
        let mut val: u32;
        let mut pmctrl: u32;
        let mut i: u32;
        let ret: usize;
        let rev: u8;
        let a_cr: bool;

        pmctrl = 0;
        rev = self.revid();
        a_cr = (rev == REV_A0 || rev == REV_A1) && self.with_cr();

        /* disable all interrupts, RXQ/TXQ */
        self.reg_write(MSIX_MASK, 0xFFFFFFFF);
        self.reg_write(IMR, 0);
        self.reg_write(ISR, ISR_DIS);

        ret = self.stop_mac();
        if (ret > 0) {
            return ret;
        }

        /* mac reset workaroud */
        self.reg_write(RFD_PIDX, 1);

        /* dis l0s/l1 before mac reset */
        if (a_cr) {
            pmctrl = self.reg_read(PMCTRL);
            if ((pmctrl & (PMCTRL_L1_EN | PMCTRL_L0S_EN)) != 0) {
                self.reg_write(PMCTRL, pmctrl & !(PMCTRL_L1_EN | PMCTRL_L0S_EN));
            }
        }

        /* reset whole mac safely */
        val = self.reg_read(MASTER);
        self.reg_write(MASTER, val | MASTER_DMA_MAC_RST | MASTER_OOB_DIS);

        /* make sure it's real idle */
        udelay(10);
        i = 0;
        while (i < DMA_MAC_RST_TO) {
            val = self.reg_read(RFD_PIDX);
            if (val == 0) {
                break;
            }
            udelay(10);
            i += 1;
        }
        while (i < DMA_MAC_RST_TO) {
            val = self.reg_read(MASTER);
            if ((val & MASTER_DMA_MAC_RST) == 0) {
                break;
            }
            udelay(10);
            i += 1;
        }
        if (i == DMA_MAC_RST_TO) {
            return ERR_RSTMAC;
        }
        udelay(10);

        if (a_cr) {
            /* set MASTER_PCLKSEL_SRDS (affect by soft-rst, PERST) */
            self.reg_write(MASTER, val | MASTER_PCLKSEL_SRDS);
            /* resoter l0s / l1 */
            if (pmctrl & (PMCTRL_L1_EN | PMCTRL_L0S_EN) > 0) {
                self.reg_write(PMCTRL, pmctrl);
            }
        }

        self.reset_osc(rev);
        /* clear Internal OSC settings, switching OSC by hw itself,
         * disable isoloate for A version
         */
        val = self.reg_read(MISC3);
        self.reg_write(MISC3, (val & !MISC3_25M_BY_SW) | MISC3_25M_NOTO_INTNL);
        val = self.reg_read(MISC);
        val &= !MISC_INTNLOSC_OPEN;
        if (rev == REV_A0 || rev == REV_A1) {
            val &= !MISC_ISO_EN;
        }
        self.reg_write(MISC, val);
        udelay(20);

        /* driver control speed/duplex, hash-alg */
        self.reg_write(MAC_CTRL, self.rx_ctrl);

        /* clk sw */
        val = self.reg_read(SERDES);
        self.reg_write(SERDES, val | SERDES_MACCLK_SLWDWN | SERDES_PHYCLK_SLWDWN);

        /* mac reset cause MDIO ctrl restore non-polling status */
        if (self.is_fpga) {
            self.start_phy_polling(MDIO_CLK_SEL_25MD128);
        }

        return ret;
    }

    unsafe fn ethadv_to_hw_cfg(&self, ethadv_cfg: u32) -> u32 {
        let mut cfg: u32 = 0;

        if (ethadv_cfg & ADVERTISED_Autoneg > 0) {
            cfg |= DRV_PHY_AUTO;
            if (ethadv_cfg & ADVERTISED_10baseT_Half > 0) {
                cfg |= DRV_PHY_10;
            }
            if (ethadv_cfg & ADVERTISED_10baseT_Full > 0) {
                cfg |= DRV_PHY_10 | DRV_PHY_DUPLEX;
            }
            if (ethadv_cfg & ADVERTISED_100baseT_Half > 0) {
                cfg |= DRV_PHY_100;
            }
            if (ethadv_cfg & ADVERTISED_100baseT_Full > 0) {
                cfg |= DRV_PHY_100 | DRV_PHY_DUPLEX;
            }
            if (ethadv_cfg & ADVERTISED_1000baseT_Half > 0) {
                cfg |= DRV_PHY_1000;
            }
            if (ethadv_cfg & ADVERTISED_1000baseT_Full > 0) {
                cfg |= DRV_PHY_100 | DRV_PHY_DUPLEX;
            }
            if (ethadv_cfg & ADVERTISED_Pause > 0) {
                cfg |= ADVERTISE_PAUSE_CAP;
            }
            if (ethadv_cfg & ADVERTISED_Asym_Pause > 0) {
                cfg |= ADVERTISE_PAUSE_ASYM;
            }
            if (self.cap & CAP_AZ > 0) {
                cfg |= DRV_PHY_EEE;
            }
        } else {
            match (ethadv_cfg) {
                ADVERTISED_10baseT_Half => {
                    cfg |= DRV_PHY_10;
                }
                ADVERTISED_100baseT_Half => {
                    cfg |= DRV_PHY_100;
                }
                ADVERTISED_10baseT_Full => {
                    cfg |= DRV_PHY_10 | DRV_PHY_DUPLEX;
                }
                ADVERTISED_100baseT_Full => {
                    cfg |= DRV_PHY_100 | DRV_PHY_DUPLEX;
                }
                _ => (),
            }
        }

        return cfg;
    }

    unsafe fn setup_speed_duplex(&mut self, ethadv: u32, flowctrl: u8) -> usize {
        let mut adv: u32;
        let mut giga: u16;
        let mut cr: u16;
        let mut val: u32;
        let mut err: usize = 0;

        /* clear flag */
        self.write_phy_reg(MII_DBG_ADDR, 0);
        val = self.reg_read(DRV);
        FIELD_SET32!(val, DRV_PHY, 0);

        if (ethadv & ADVERTISED_Autoneg > 0) {
            adv = ADVERTISE_CSMA;
            adv |= ethtool_adv_to_mii_adv_t(ethadv);

            if (flowctrl & FC_ANEG == FC_ANEG) {
                if (flowctrl & FC_RX > 0) {
                    adv |= ADVERTISED_Pause;
                    if (flowctrl & FC_TX == 0) {
                        adv |= ADVERTISED_Asym_Pause;
                    }
                } else if (flowctrl & FC_TX > 0) {
                    adv |= ADVERTISED_Asym_Pause;
                }
            }
            giga = 0;
            if (self.cap & CAP_GIGA > 0) {
                giga = ethtool_adv_to_mii_ctrl1000_t(ethadv) as u16;
            }

            cr = BMCR_RESET | BMCR_ANENABLE | BMCR_ANRESTART;

            if (self.write_phy_reg(MII_ADVERTISE, adv as u16) > 0
                || self.write_phy_reg(MII_CTRL1000, giga) > 0
                || self.write_phy_reg(MII_BMCR, cr) > 0)
            {
                err = ERR_MIIBUSY;
            }
        } else {
            cr = BMCR_RESET;
            if (ethadv == ADVERTISED_100baseT_Half || ethadv == ADVERTISED_100baseT_Full) {
                cr |= BMCR_SPEED100;
            }
            if (ethadv == ADVERTISED_10baseT_Full || ethadv == ADVERTISED_100baseT_Full) {
                cr |= BMCR_FULLDPLX;
            }

            err = self.write_phy_reg(MII_BMCR, cr);
        }

        if (err == 0) {
            self.write_phy_reg(MII_DBG_ADDR, PHY_INITED);
            /* save config to HW */
            val |= self.ethadv_to_hw_cfg(ethadv);
        }

        self.reg_write(DRV, val);

        return err;
    }

    unsafe fn get_perm_macaddr(&mut self) -> [u8; 6] {
        let mac_low = self.reg_read(STAD0);
        let mac_high = self.reg_read(STAD1);
        [
            mac_low as u8,
            (mac_low >> 8) as u8,
            (mac_low >> 16) as u8,
            (mac_low >> 24) as u8,
            mac_high as u8,
            (mac_high >> 8) as u8,
        ]
    }

    unsafe fn get_phy_link(&mut self, link_up: &mut bool, speed: &mut u16) -> usize {
        let mut bmsr: u16 = 0;
        let mut giga: u16 = 0;
        let mut err: usize;

        self.read_phy_reg(MII_BMSR, &mut bmsr);
        err = self.read_phy_reg(MII_BMSR, &mut bmsr);
        if (err > 0) {
            return err;
        }

        if (bmsr & BMSR_LSTATUS == 0) {
            *link_up = false;
            return err;
        }

        *link_up = true;

        /* speed/duplex result is saved in PHY Specific Status Register */
        err = self.read_phy_reg(MII_GIGA_PSSR, &mut giga);
        if (err > 0) {
            return err;
        }

        if (giga & GIGA_PSSR_SPD_DPLX_RESOLVED == 0) {
            println!("PHY SPD/DPLX unresolved: {:X}", giga);
            err = (-EINVAL) as usize;
        } else {
            match (giga & GIGA_PSSR_SPEED) {
                GIGA_PSSR_1000MBS => *speed = SPEED_1000,
                GIGA_PSSR_100MBS => *speed = SPEED_100,
                GIGA_PSSR_10MBS => *speed = SPEED_10,
                _ => {
                    println!("PHY SPD/DPLX unresolved: {:X}", giga);
                    err = (-EINVAL) as usize;
                }
            }
            *speed += if (giga & GIGA_PSSR_DPLX > 0) {
                FULL_DUPLEX as u16
            } else {
                HALF_DUPLEX as u16
            };
        }

        return err;
    }

    fn show_speed(&self, speed: u16) {
        let desc = if speed == SPEED_1000 + FULL_DUPLEX as u16 {
            "1 Gbps Full"
        } else if speed == SPEED_100 + FULL_DUPLEX as u16 {
            "100 Mbps Full"
        } else if speed == SPEED_100 + HALF_DUPLEX as u16 {
            "100 Mbps Half"
        } else if speed == SPEED_10 + FULL_DUPLEX as u16 {
            "10 Mbps Full"
        } else if speed == SPEED_10 + HALF_DUPLEX as u16 {
            "10 Mbps Half"
        } else {
            "Unknown speed"
        };

        println!("NIC Link Up: {}", desc);
    }

    unsafe fn configure_basic(&mut self) {
        let mut val: u32;
        let raw_mtu: u32;
        let max_payload: u32;
        let val16: u16;
        let chip_rev = self.revid();

        /* mac address */
        //TODO alx_set_macaddr(hw, self.mac_addr);

        /* clk gating */
        self.reg_write(CLK_GATE, CLK_GATE_ALL_A0);

        /* idle timeout to switch clk_125M */
        if (chip_rev >= REV_B0) {
            self.reg_write(IDLE_DECISN_TIMER, IDLE_DECISN_TIMER_DEF);
        }

        /* stats refresh timeout */
        self.reg_write(SMB_TIMER, self.smb_timer * 500);

        /* intr moduration */
        val = self.reg_read(MASTER);
        val = val | MASTER_IRQMOD2_EN | MASTER_IRQMOD1_EN | MASTER_SYSALVTIMER_EN;
        self.reg_write(MASTER, val);
        self.reg_write(IRQ_MODU_TIMER, FIELDX!(IRQ_MODU_TIMER1, self.imt >> 1));
        /* intr re-trig timeout */
        self.reg_write(INT_RETRIG, INT_RETRIG_TO);
        /* tpd threshold to trig int */
        self.reg_write(TINT_TPD_THRSHLD, self.ith_tpd);
        self.reg_write(TINT_TIMER, self.imt as u32);

        /* mtu, 8:fcs+vlan */
        raw_mtu = (self.mtu + ETH_HLEN) as u32;
        self.reg_write(MTU, raw_mtu + 8);
        if (raw_mtu > MTU_JUMBO_TH) {
            self.rx_ctrl &= !MAC_CTRL_FAST_PAUSE;
        }

        /* txq */
        if ((raw_mtu + 8) < TXQ1_JUMBO_TSO_TH) {
            val = (raw_mtu + 8 + 7) >> 3;
        } else {
            val = TXQ1_JUMBO_TSO_TH >> 3;
        }
        self.reg_write(TXQ1, val | TXQ1_ERRLGPKT_DROP_EN);

        /* TODO
        max_payload = alx_get_readrq(hw) >> 8;
        /*
         * if BIOS had changed the default dma read max length,
         * restore it to default value
         */
        if (max_payload < DEV_CTRL_MAXRRS_MIN)
            alx_set_readrq(hw, 128 << DEV_CTRL_MAXRRS_MIN);
        */
        max_payload = 128 << DEV_CTRL_MAXRRS_MIN;

        val = FIELDX!(TXQ0_TPD_BURSTPREF, TXQ_TPD_BURSTPREF_DEF)
            | TXQ0_MODE_ENHANCE
            | TXQ0_LSO_8023_EN
            | TXQ0_SUPT_IPOPT
            | FIELDX!(TXQ0_TXF_BURST_PREF, TXQ_TXF_BURST_PREF_DEF);
        self.reg_write(TXQ0, val);
        val = FIELDX!(HQTPD_Q1_NUMPREF, TXQ_TPD_BURSTPREF_DEF)
            | FIELDX!(HQTPD_Q2_NUMPREF, TXQ_TPD_BURSTPREF_DEF)
            | FIELDX!(HQTPD_Q3_NUMPREF, TXQ_TPD_BURSTPREF_DEF)
            | HQTPD_BURST_EN;
        self.reg_write(HQTPD, val);

        /* rxq, flow control */
        val = self.reg_read(SRAM5);
        val = FIELD_GETX!(val, SRAM_RXF_LEN) << 3;
        if (val > SRAM_RXF_LEN_8K) {
            val16 = (MTU_STD_ALGN >> 3) as u16;
            val = (val - RXQ2_RXF_FLOW_CTRL_RSVD) >> 3;
        } else {
            val16 = (MTU_STD_ALGN >> 3) as u16;
            val = (val - MTU_STD_ALGN) >> 3;
        }
        self.reg_write(
            RXQ2,
            FIELDX!(RXQ2_RXF_XOFF_THRESH, val16) | FIELDX!(RXQ2_RXF_XON_THRESH, val),
        );
        val = FIELDX!(RXQ0_NUM_RFD_PREF, RXQ0_NUM_RFD_PREF_DEF)
            | FIELDX!(RXQ0_RSS_MODE, RXQ0_RSS_MODE_DIS)
            | FIELDX!(RXQ0_IDT_TBL_SIZE, RXQ0_IDT_TBL_SIZE_DEF)
            | RXQ0_RSS_HSTYP_ALL
            | RXQ0_RSS_HASH_EN
            | RXQ0_IPV6_PARSE_EN;
        if (self.cap & CAP_GIGA > 0) {
            FIELD_SET32!(val, RXQ0_ASPM_THRESH, RXQ0_ASPM_THRESH_100M);
        }
        self.reg_write(RXQ0, val);

        /* DMA */
        self.reg_read(DMA);

        val = FIELDX!(DMA_RORDER_MODE, DMA_RORDER_MODE_OUT)
            | DMA_RREQ_PRI_DATA
            | FIELDX!(DMA_RREQ_BLEN, max_payload)
            | FIELDX!(DMA_WDLY_CNT, DMA_WDLY_CNT_DEF)
            | FIELDX!(DMA_RDLY_CNT, DMA_RDLY_CNT_DEF)
            | FIELDX!(DMA_RCHNL_SEL, self.dma_chnl - 1);
        self.reg_write(DMA, val);

        /* multi-tx-q weight */
        if (self.cap & CAP_MTQ > 0) {
            val = FIELDX!(WRR_PRI, self.wrr_ctrl)
                | FIELDX!(WRR_PRI0, self.wrr[0])
                | FIELDX!(WRR_PRI1, self.wrr[1])
                | FIELDX!(WRR_PRI2, self.wrr[2])
                | FIELDX!(WRR_PRI3, self.wrr[3]);
            self.reg_write(WRR, val);
        }
    }

    unsafe fn set_rx_mode(&mut self) {
        /* TODO
        struct alx_adapter *adpt = netdev_priv(netdev);
        struct alx_hw *hw = &adpt->hw;
        struct netdev_hw_addr *ha;


        /* comoute mc addresses' hash value ,and put it into hash table */
        netdev_for_each_mc_addr(ha, netdev)
            alx_add_mc_addr(hw, ha->addr);
        */

        self.reg_write(HASH_TBL0, self.mc_hash[0]);
        self.reg_write(HASH_TBL1, self.mc_hash[1]);

        /* check for Promiscuous and All Multicast modes */
        self.rx_ctrl &= !(MAC_CTRL_MULTIALL_EN | MAC_CTRL_PROMISC_EN);
        /* TODO
        if (netdev->flags & IFF_PROMISC) {
            self.rx_ctrl |= MAC_CTRL_PROMISC_EN;
        }
        if (netdev->flags & IFF_ALLMULTI) {
            self.rx_ctrl |= MAC_CTRL_MULTIALL_EN;
        }
        */

        self.reg_write(MAC_CTRL, self.rx_ctrl);
    }

    unsafe fn set_vlan_mode(&mut self, vlan_rx: bool) {
        if (vlan_rx) {
            self.rx_ctrl |= MAC_CTRL_VLANSTRIP;
        } else {
            self.rx_ctrl &= !MAC_CTRL_VLANSTRIP;
        }

        self.reg_write(MAC_CTRL, self.rx_ctrl);
    }

    unsafe fn configure_rss(&mut self, en: bool) {
        let mut ctrl: u32;

        ctrl = self.reg_read(RXQ0);

        if (en) {
            unimplemented!();
            /*
            for (i = 0; i < sizeof(self.rss_key); i++) {
                /* rss key should be saved in chip with
                 * reversed order.
                 */
                int j = sizeof(self.rss_key) - i - 1;

                MEM_W8(hw, RSS_KEY0 + j, self.rss_key[i]);
            }

            for (i = 0; i < ARRAY_SIZE(self.rss_idt); i++)
                self.reg_write(RSS_IDT_TBL0 + i * 4,
                        self.rss_idt[i]);

            FIELD_SET32(ctrl, RXQ0_RSS_HSTYP, self.rss_hash_type);
            FIELD_SET32(ctrl, RXQ0_RSS_MODE, RXQ0_RSS_MODE_MQMI);
            FIELD_SET32(ctrl, RXQ0_IDT_TBL_SIZE, self.rss_idt_size);
            ctrl |= RXQ0_RSS_HASH_EN;
            */
        } else {
            ctrl &= !RXQ0_RSS_HASH_EN;
        }

        self.reg_write(RXQ0, ctrl);
    }

    unsafe fn configure(&mut self) {
        self.configure_basic();
        self.configure_rss(false);
        self.set_rx_mode();
        self.set_vlan_mode(false);
    }

    unsafe fn irq_enable(&mut self) {
        self.reg_write(ISR, 0);
        let imask = self.imask;
        self.reg_write(IMR, imask);
    }

    unsafe fn irq_disable(&mut self) {
        self.reg_write(ISR, ISR_DIS);
        self.reg_write(IMR, 0);
    }

    unsafe fn clear_phy_intr(&mut self) -> usize {
        let mut isr: u16 = 0;
        self.read_phy_reg(MII_ISR, &mut isr)
    }

    unsafe fn post_phy_link(&mut self, speed: u16, az_en: bool) {
        let mut phy_val: u16 = 0;
        let len: u16;
        let agc: u16;
        let revid: u8 = self.revid();
        let adj_th: bool;

        if (revid != REV_B0 && revid != REV_A1 && revid != REV_A0) {
            return;
        }
        adj_th = if (revid == REV_B0) { true } else { false };

        /* 1000BT/AZ, wrong cable length */
        if (speed != SPEED_0) {
            self.read_phy_ext(MIIEXT_PCS, MIIEXT_CLDCTRL6, &mut phy_val);
            len = FIELD_GETX!(phy_val, CLDCTRL6_CAB_LEN);
            self.read_phy_dbg(MIIDBG_AGC, &mut phy_val);
            agc = FIELD_GETX!(phy_val, AGC_2_VGA);

            if ((speed == SPEED_1000
                && (len > CLDCTRL6_CAB_LEN_SHORT1G || (0 == len && agc > AGC_LONG1G_LIMT)))
                || (speed == SPEED_100
                    && (len > CLDCTRL6_CAB_LEN_SHORT100M || (0 == len && agc > AGC_LONG100M_LIMT))))
            {
                self.write_phy_dbg(MIIDBG_AZ_ANADECT, AZ_ANADECT_LONG);
                self.read_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, &mut phy_val);
                self.write_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, phy_val | AFE_10BT_100M_TH);
            } else {
                self.write_phy_dbg(MIIDBG_AZ_ANADECT, AZ_ANADECT_DEF);
                self.read_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, &mut phy_val);
                self.write_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, phy_val & !AFE_10BT_100M_TH);
            }

            /* threashold adjust */
            if (adj_th && self.lnk_patch) {
                if (speed == SPEED_100) {
                    self.write_phy_dbg(MIIDBG_MSE16DB, MSE16DB_UP);
                } else if (speed == SPEED_1000) {
                    /*
                     * Giga link threshold, raise the tolerance of
                     * noise 50%
                     */
                    self.read_phy_dbg(MIIDBG_MSE20DB, &mut phy_val);
                    FIELD_SETS!(phy_val, MSE20DB_TH, MSE20DB_TH_HI);
                    self.write_phy_dbg(MIIDBG_MSE20DB, phy_val);
                }
            }
            /* phy link-down in 1000BT/AZ mode */
            if (az_en && revid == REV_B0 && speed == SPEED_1000) {
                self.write_phy_dbg(MIIDBG_SRDSYSMOD, SRDSYSMOD_DEF & !SRDSYSMOD_DEEMP_EN);
            }
        } else {
            self.read_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, &mut phy_val);
            self.write_phy_ext(MIIEXT_ANEG, MIIEXT_AFE, phy_val & !AFE_10BT_100M_TH);

            if (adj_th && self.lnk_patch) {
                self.write_phy_dbg(MIIDBG_MSE16DB, MSE16DB_DOWN);
                self.read_phy_dbg(MIIDBG_MSE20DB, &mut phy_val);
                FIELD_SETS!(phy_val, MSE20DB_TH, MSE20DB_TH_DEF);
                self.write_phy_dbg(MIIDBG_MSE20DB, phy_val);
            }
            if (az_en && revid == REV_B0) {
                self.write_phy_dbg(MIIDBG_SRDSYSMOD, SRDSYSMOD_DEF);
            }
        }
    }

    unsafe fn task(&mut self) {
        if self.flag & FLAG_HALT > 0 {
            return;
        }

        //TODO: RESET
        if self.flag & FLAG_TASK_RESET > 0 {
            self.flag &= !FLAG_TASK_RESET;
            println!("reinit");
            self.reinit();
        }

        if self.flag & FLAG_TASK_CHK_LINK > 0 {
            self.flag &= !FLAG_TASK_CHK_LINK;
            self.check_link();
        }
    }

    unsafe fn halt(&mut self) {
        self.flag |= FLAG_HALT;

        //alx_netif_stop(adpt);
        self.link_up = false;
        self.link_speed = SPEED_0;

        self.reset_mac();

        /* disable l0s/l1 */
        self.enable_aspm(false, false);
        self.irq_disable();
        //self.free_all_rings_buf();
    }

    unsafe fn activate(&mut self) {
        /* hardware setting lost, restore it */
        self.init_ring_ptrs();
        self.configure();

        self.flag &= !FLAG_HALT;
        /* clear old interrupts */
        self.reg_write(ISR, !ISR_DIS);

        self.irq_enable();

        self.flag |= FLAG_TASK_CHK_LINK;
        self.task();
    }

    unsafe fn reinit(&mut self) {
        if self.flag & FLAG_HALT > 0 {
            return;
        }

        self.halt();
        self.activate();
    }

    unsafe fn init_ring_ptrs(&mut self) {
        // Write high addresses
        self.reg_write(RX_BASE_ADDR_HI, 0);
        self.reg_write(TX_BASE_ADDR_HI, 0);

        // RFD ring
        for i in 0..self.rfd_ring.len() {
            self.rfd_ring[i]
                .addr_low
                .write(self.rfd_buffer[i].physical() as u32);
            self.rfd_ring[i]
                .addr_high
                .write(((self.rfd_buffer[i].physical() as u64) >> 32) as u32);
        }
        self.reg_write(RFD_ADDR_LO, self.rfd_ring.physical() as u32);
        self.reg_write(RFD_RING_SZ, self.rfd_ring.len() as u32);
        self.reg_write(RFD_BUF_SZ, 16384);

        // RRD ring
        self.reg_write(RRD_ADDR_LO, self.rrd_ring.physical() as u32);
        self.reg_write(RRD_RING_SZ, self.rrd_ring.len() as u32);

        // TPD ring
        self.reg_write(TPD_PRI0_ADDR_LO, self.tpd_ring[0].physical() as u32);
        self.reg_write(TPD_PRI1_ADDR_LO, self.tpd_ring[1].physical() as u32);
        self.reg_write(TPD_PRI2_ADDR_LO, self.tpd_ring[2].physical() as u32);
        self.reg_write(TPD_PRI3_ADDR_LO, self.tpd_ring[3].physical() as u32);
        self.reg_write(TPD_RING_SZ, self.tpd_ring[0].len() as u32);

        // Write pointers into chip SRAM
        self.reg_write(SRAM9, SRAM_LOAD_PTR);
    }

    unsafe fn check_link(&mut self) {
        let mut speed: u16 = SPEED_0;
        let old_speed: u16;
        let mut link_up: bool = false;
        let old_link_up: bool;
        let mut err: usize;

        if (self.flag & FLAG_HALT > 0) {
            return;
        }

        macro_rules! goto_out {
            () => {
                if (err > 0) {
                    self.flag |= FLAG_TASK_RESET;
                    self.task();
                }
                return;
            };
        }

        /* clear PHY internal interrupt status,
         * otherwise the Main interrupt status will be asserted
         * for ever.
         */
        self.clear_phy_intr();

        err = self.get_phy_link(&mut link_up, &mut speed);
        if (err > 0) {
            goto_out!();
        }

        /* open interrutp mask */
        self.imask |= ISR_PHY;
        let imask = self.imask;
        self.reg_write(IMR, imask);

        if (!link_up && !self.link_up) {
            goto_out!();
        }

        old_speed = self.link_speed + self.link_duplex as u16;
        old_link_up = self.link_up;

        if (link_up) {
            /* same speed ? */
            if (old_link_up && old_speed == speed) {
                goto_out!();
            }

            self.show_speed(speed);
            self.link_duplex = (speed % 10) as u8;
            self.link_speed = speed - self.link_duplex as u16;
            self.link_up = true;
            let link_speed = self.link_speed;
            let az_en = self.cap & CAP_AZ > 0;
            self.post_phy_link(link_speed, az_en);
            let l0s_en = self.cap & CAP_L0S > 0;
            let l1_en = self.cap & CAP_L1 > 0;
            self.enable_aspm(l0s_en, l1_en);
            self.start_mac();

            /* link kept, just speed changed */
            if (old_link_up) {
                goto_out!();
            }
            /* link changed from 'down' to 'up' */
            // TODO self.netif_start();
            goto_out!();
        }

        /* link changed from 'up' to 'down' */
        // TODO self.netif_stop();
        self.link_up = false;
        self.link_speed = SPEED_0;
        println!("NIC Link Down");
        err = self.reset_mac();
        if (err > 0) {
            println!("linkdown:reset_mac fail {}", err);
            err = (-EIO) as usize;
            goto_out!();
        }
        self.irq_disable();

        /* reset-mac cause all settings on HW lost,
         * following steps restore all of them and
         * refresh whole RX/TX rings
         */
        self.init_ring_ptrs();

        self.configure();

        let l1_en = self.cap & CAP_L1 > 0;
        self.enable_aspm(false, l1_en);

        let cap_az = self.cap & CAP_AZ > 0;
        self.post_phy_link(SPEED_0, cap_az);

        self.irq_enable();

        goto_out!();
    }

    unsafe fn get_phy_info(&mut self) -> bool {
        /*
        let mut devs1: u16 = 0;
        let mut devs2: u16 = 0;

        if (self.read_phy_reg(MII_PHYSID1, &mut self.phy_id[0]) > 0 ||
            self.read_phy_reg(MII_PHYSID2, &mut self.phy_id[1]) > 0) {
            return false;
        }

        /* since we haven't PMA/PMD status2 register, we can't
         * use mdio45_probe function for prtad and mmds.
         * use fixed MMD3 to get mmds.
         */
        if (self.read_phy_ext(3, MDIO_DEVS1, &devs1) ||
            self.read_phy_ext(3, MDIO_DEVS2, &devs2)) {
            return false;
        }
        self.mdio.mmds = devs1 | devs2 << 16;

        return true;
        */
        return true;
    }

    unsafe fn probe(&mut self) -> Result<()> {
        println!("   - Reset PCIE");
        self.reset_pcie();

        println!("   - Reset PHY");
        self.reset_phy();

        println!("   - Reset MAC");
        let err = self.reset_mac();
        if err > 0 {
            println!("   - MAC reset failed: {}", err);
            return Err(Error::new(EIO));
        }

        println!("   - Setup speed duplex");
        let ethadv = self.adv_cfg;
        let flowctrl = self.flowctrl;
        let err = self.setup_speed_duplex(ethadv, flowctrl);
        if err > 0 {
            println!("   - PHY speed/duplex failed: {}", err);
            return Err(Error::new(EIO));
        }

        let mac = self.get_perm_macaddr();
        println!(
            "   - MAC: {:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}\n",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        if !self.get_phy_info() {
            println!("   - Identify PHY failed");
            return Err(Error::new(EIO));
        }

        Ok(())
    }

    unsafe fn free_all_ring_resources(&mut self) {
        println!("free_all_ring_resources");
    }

    unsafe fn disable_advanced_intr(&mut self) {
        println!("disable_advanced_intr");
    }

    unsafe fn open(&mut self) -> usize {
        /* allocate all memory resources */
        self.init_ring_ptrs();

        /* make hardware ready before allocate interrupt */
        self.configure();

        self.flag &= !FLAG_HALT;

        /* clear old interrupts */
        self.reg_write(ISR, !ISR_DIS);

        self.irq_enable();

        self.flag |= FLAG_TASK_CHK_LINK;
        self.task();
        return 0;
    }

    unsafe fn init(&mut self) -> Result<()> {
        {
            let pci_id = self.reg_read(0);
            self.vendor_id = pci_id as u16;
            self.device_id = (pci_id >> 16) as u16;
        }

        {
            let pci_subid = self.reg_read(0x2C);
            self.subven_id = pci_subid as u16;
            self.subdev_id = (pci_subid >> 16) as u16;
        }

        {
            let pci_rev = self.reg_read(8);
            self.revision = pci_rev as u8;
        }

        {
            self.dma_chnl = if self.revid() >= REV_B0 { 4 } else { 2 };
        }

        println!(
            "   - ID: {:>04X}:{:>04X} SUB: {:>04X}:{:>04X} REV: {:>02X}",
            self.vendor_id, self.device_id, self.subven_id, self.subdev_id, self.revision
        );

        self.probe()?;

        let err = self.open();
        if err > 0 {
            println!("   - Failed to open: {}", err);
            return Err(Error::new(EIO));
        }

        Ok(())
    }
}

impl SchemeSync for Alx {
    fn open(&mut self, path: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid == 0 {
            Ok(OpenResult::ThisScheme {
                number: flags,
                flags: NewFdFlags::empty(),
            })
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        /*
        let head = unsafe { self.reg_read(RDH) };
        let mut tail = unsafe { self.reg_read(RDT) };

        tail += 1;
        if tail >= self.receive_ring.len() as u32 {
            tail = 0;
        }

        if tail != head {
            let rd = unsafe { &mut * (self.receive_ring.as_ptr().offset(tail as isize) as *mut Rd) };
            if rd.status & RD_DD == RD_DD {
                rd.status = 0;

                let data = &self.receive_buffer[tail as usize][.. rd.length as usize];

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    buf[i] = data[i];
                    i += 1;
                }

                unsafe { self.reg_write(RDT, tail) };

                return Ok(i);
            }
        }
        */

        if id & O_NONBLOCK == O_NONBLOCK {
            Ok(0)
        } else {
            Err(Error::new(EWOULDBLOCK))
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
        /*
        loop {
            let head = unsafe { self.reg_read(TDH) };
            let mut tail = unsafe { self.reg_read(TDT) };
            let old_tail = tail;

            tail += 1;
            if tail >= self.transmit_ring.len() as u32 {
                tail = 0;
            }

            if tail != head {
                let td = unsafe { &mut * (self.transmit_ring.as_ptr().offset(old_tail as isize) as *mut Td) };

                td.cso = 0;
                td.command = TD_CMD_EOP | TD_CMD_IFCS | TD_CMD_RS;
                td.status = 0;
                td.css = 0;
                td.special = 0;

                td.length = (cmp::min(buf.len(), 0x3FFF)) as u16;

                let mut data = unsafe { slice::from_raw_parts_mut(self.transmit_buffer[old_tail as usize].as_ptr() as *mut u8, td.length as usize) };

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    data[i] = buf[i];
                    i += 1;
                }

                unsafe { self.reg_write(TDT, tail) };

                while td.status == 0 {
                    thread::yield_now();
                }

                return Ok(i);
            }
        }
        */
        Ok(0)
    }

    fn fevent(&mut self, _id: usize, _flags: EventFlags, _ctx: &CallerCtx) -> Result<EventFlags> {
        Ok(EventFlags::empty())
    }

    fn fsync(&mut self, _id: usize, _ctx: &CallerCtx) -> Result<()> {
        Ok(())
    }

    fn on_close(&mut self, _id: usize) {}
}
