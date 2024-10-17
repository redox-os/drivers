/*
 * Copyright (c) 2017 Jeremy Soller
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF;
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 * ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 * ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF;
 * OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */
/*
 * Copyright (c) 2012 Qualcomm Atheros, Inc.
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF;
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 * ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 * ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF;
 * OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

/**********************************************************************/
/* following registers are mapped to both pci config and memory space */
/**********************************************************************/

/* pci dev-ids */
pub const DEV_ID_AR8161: u32 = 0x1091;
pub const DEV_ID_AR8162: u32 = 0x1090;
pub const DEV_ID_AR8171: u32 = 0x10A1;
pub const DEV_ID_AR8172: u32 = 0x10A0;

/* rev definition,
 * bit(0): with xD support
 * bit(1): with Card Reader function
 * bit(7:2): real revision
 */
pub const PCI_REVID_WTH_CR: u32 = 1 << 1;
pub const PCI_REVID_WTH_XD: u32 = 1 << 0;
pub const PCI_REVID_MASK: u32 = 0x1F;
pub const PCI_REVID_SHIFT: u32 = 3;
pub const REV_A0: u8 = 0;
pub const REV_A1: u8 = 1;
pub const REV_B0: u8 = 2;
pub const REV_C0: u8 = 3;

pub const PM_CSR: u32 = 0x0044;
pub const PM_CSR_PME_STAT: u32 = 1 << 15;
pub const PM_CSR_DSCAL_MASK: u32 = 0x3;
pub const PM_CSR_DSCAL_SHIFT: u32 = 13;
pub const PM_CSR_DSEL_MASK: u32 = 0xF;
pub const PM_CSR_DSEL_SHIFT: u32 = 9;
pub const PM_CSR_PME_EN: u32 = 1 << 8;
pub const PM_CSR_PWST_MASK: u32 = 0x3;
pub const PM_CSR_PWST_SHIFT: u32 = 0;

pub const DEV_CAP: u32 = 0x005C;
pub const DEV_CAP_SPLSL_MASK: u32 = 0x3;
pub const DEV_CAP_SPLSL_SHIFT: u32 = 26;
pub const DEV_CAP_SPLV_MASK: u32 = 0xFF;
pub const DEV_CAP_SPLV_SHIFT: u32 = 18;
pub const DEV_CAP_RBER: u32 = 1 << 15;
pub const DEV_CAP_PIPRS: u32 = 1 << 14;
pub const DEV_CAP_AIPRS: u32 = 1 << 13;
pub const DEV_CAP_ABPRS: u32 = 1 << 12;
pub const DEV_CAP_L1ACLAT_MASK: u32 = 0x7;
pub const DEV_CAP_L1ACLAT_SHIFT: u32 = 9;
pub const DEV_CAP_L0SACLAT_MASK: u32 = 0x7;
pub const DEV_CAP_L0SACLAT_SHIFT: u32 = 6;
pub const DEV_CAP_EXTAG: u32 = 1 << 5;
pub const DEV_CAP_PHANTOM: u32 = 1 << 4;
pub const DEV_CAP_MPL_MASK: u32 = 0x7;
pub const DEV_CAP_MPL_SHIFT: u32 = 0;
pub const DEV_CAP_MPL_128: u32 = 1;
pub const DEV_CAP_MPL_256: u32 = 2;
pub const DEV_CAP_MPL_512: u32 = 3;
pub const DEV_CAP_MPL_1024: u32 = 4;
pub const DEV_CAP_MPL_2048: u32 = 5;
pub const DEV_CAP_MPL_4096: u32 = 6;

pub const DEV_CTRL: u32 = 0x0060;
pub const DEV_CTRL_MAXRRS_MASK: u32 = 0x7;
pub const DEV_CTRL_MAXRRS_SHIFT: u32 = 12;
pub const DEV_CTRL_MAXRRS_MIN: u32 = 2;
pub const DEV_CTRL_NOSNP_EN: u32 = 1 << 11;
pub const DEV_CTRL_AUXPWR_EN: u32 = 1 << 10;
pub const DEV_CTRL_PHANTOM_EN: u32 = 1 << 9;
pub const DEV_CTRL_EXTAG_EN: u32 = 1 << 8;
pub const DEV_CTRL_MPL_MASK: u32 = 0x7;
pub const DEV_CTRL_MPL_SHIFT: u32 = 5;
pub const DEV_CTRL_RELORD_EN: u32 = 1 << 4;
pub const DEV_CTRL_URR_EN: u32 = 1 << 3;
pub const DEV_CTRL_FERR_EN: u32 = 1 << 2;
pub const DEV_CTRL_NFERR_EN: u32 = 1 << 1;
pub const DEV_CTRL_CERR_EN: u32 = 1 << 0;

pub const DEV_STAT: u32 = 0x0062;
pub const DEV_STAT_XS_PEND: u32 = 1 << 5;
pub const DEV_STAT_AUXPWR: u32 = 1 << 4;
pub const DEV_STAT_UR: u32 = 1 << 3;
pub const DEV_STAT_FERR: u32 = 1 << 2;
pub const DEV_STAT_NFERR: u32 = 1 << 1;
pub const DEV_STAT_CERR: u32 = 1 << 0;

pub const LNK_CAP: u32 = 0x0064;
pub const LNK_CAP_PRTNUM_MASK: u32 = 0xFF;
pub const LNK_CAP_PRTNUM_SHIFT: u32 = 24;
pub const LNK_CAP_CLK_PM: u32 = 1 << 18;
pub const LNK_CAP_L1EXTLAT_MASK: u32 = 0x7;
pub const LNK_CAP_L1EXTLAT_SHIFT: u32 = 15;
pub const LNK_CAP_L0SEXTLAT_MASK: u32 = 0x7;
pub const LNK_CAP_L0SEXTLAT_SHIFT: u32 = 12;
pub const LNK_CAP_ASPM_SUP_MASK: u32 = 0x3;
pub const LNK_CAP_ASPM_SUP_SHIFT: u32 = 10;
pub const LNK_CAP_ASPM_SUP_L0S: u32 = 1;
pub const LNK_CAP_ASPM_SUP_L0SL1: u32 = 3;
pub const LNK_CAP_MAX_LWH_MASK: u32 = 0x3F;
pub const LNK_CAP_MAX_LWH_SHIFT: u32 = 4;
pub const LNK_CAP_MAX_LSPD_MASK: u32 = 0xF;
pub const LNK_CAP_MAX_LSPD_SHIFT: u32 = 0;

pub const LNK_CTRL: u32 = 0x0068;
pub const LNK_CTRL_CLK_PM_EN: u32 = 1 << 8;
pub const LNK_CTRL_EXTSYNC: u32 = 1 << 7;
pub const LNK_CTRL_CMNCLK_CFG: u32 = 1 << 6;
pub const LNK_CTRL_RCB_128B: u32 = 1 << 3;
pub const LNK_CTRL_ASPM_MASK: u32 = 0x3;
pub const LNK_CTRL_ASPM_SHIFT: u32 = 0;
pub const LNK_CTRL_ASPM_DIS: u32 = 0;
pub const LNK_CTRL_ASPM_ENL0S: u32 = 1;
pub const LNK_CTRL_ASPM_ENL1: u32 = 2;
pub const LNK_CTRL_ASPM_ENL0SL1: u32 = 3;

pub const LNK_STAT: u32 = 0x006A;
pub const LNK_STAT_SCLKCFG: u32 = 1 << 12;
pub const LNK_STAT_LNKTRAIN: u32 = 1 << 11;
pub const LNK_STAT_TRNERR: u32 = 1 << 10;
pub const LNK_STAT_LNKSPD_MASK: u32 = 0xF;
pub const LNK_STAT_LNKSPD_SHIFT: u32 = 0;
pub const LNK_STAT_NEGLW_MASK: u32 = 0x3F;
pub const LNK_STAT_NEGLW_SHIFT: u32 = 4;

pub const MSIX_MASK: u32 = 0x0090;
pub const MSIX_PENDING: u32 = 0x0094;

pub const UE_SVRT: u32 = 0x010C;
pub const UE_SVRT_UR: u32 = 1 << 20;
pub const UE_SVRT_ECRCERR: u32 = 1 << 19;
pub const UE_SVRT_MTLP: u32 = 1 << 18;
pub const UE_SVRT_RCVOVFL: u32 = 1 << 17;
pub const UE_SVRT_UNEXPCPL: u32 = 1 << 16;
pub const UE_SVRT_CPLABRT: u32 = 1 << 15;
pub const UE_SVRT_CPLTO: u32 = 1 << 14;
pub const UE_SVRT_FCPROTERR: u32 = 1 << 13;
pub const UE_SVRT_PTLP: u32 = 1 << 12;
pub const UE_SVRT_DLPROTERR: u32 = 1 << 4;
pub const UE_SVRT_TRNERR: u32 = 1 << 0;

/* eeprom & flash load register */
pub const EFLD: u32 = 0x0204;
pub const EFLD_F_ENDADDR_MASK: u32 = 0x3FF;
pub const EFLD_F_ENDADDR_SHIFT: u32 = 16;
pub const EFLD_F_EXIST: u32 = 1 << 10;
pub const EFLD_E_EXIST: u32 = 1 << 9;
pub const EFLD_EXIST: u32 = 1 << 8;
pub const EFLD_STAT: u32 = 1 << 5;
pub const EFLD_IDLE: u32 = 1 << 4;
pub const EFLD_START: u32 = 1 << 0;

/* eFuse load register */
pub const SLD: u32 = 0x0218;
pub const SLD_FREQ_MASK: u32 = 0x3;
pub const SLD_FREQ_SHIFT: u32 = 24;
pub const SLD_FREQ_100K: u32 = 0;
pub const SLD_FREQ_200K: u32 = 1;
pub const SLD_FREQ_300K: u32 = 2;
pub const SLD_FREQ_400K: u32 = 3;
pub const SLD_EXIST: u32 = 1 << 23;
pub const SLD_SLVADDR_MASK: u32 = 0x7F;
pub const SLD_SLVADDR_SHIFT: u32 = 16;
pub const SLD_IDLE: u32 = 1 << 13;
pub const SLD_STAT: u32 = 1 << 12;
pub const SLD_START: u32 = 1 << 11;
pub const SLD_STARTADDR_MASK: u32 = 0xFF;
pub const SLD_STARTADDR_SHIFT: u32 = 0;
pub const SLD_MAX_TO: u32 = 100;

pub const PCIE_MSIC: u32 = 0x021C;
pub const PCIE_MSIC_MSIX_DIS: u32 = 1 << 22;
pub const PCIE_MSIC_MSI_DIS: u32 = 1 << 21;

pub const PPHY_MISC1: u32 = 0x1000;
pub const PPHY_MISC1_RCVDET: u32 = 1 << 2;
pub const PPHY_MISC1_NFTS_MASK: u32 = 0xFF;
pub const PPHY_MISC1_NFTS_SHIFT: u32 = 16;
pub const PPHY_MISC1_NFTS_HIPERF: u32 = 0xA0;

pub const PPHY_MISC2: u32 = 0x1004;
pub const PPHY_MISC2_L0S_TH_MASK: u32 = 0x3;
pub const PPHY_MISC2_L0S_TH_SHIFT: u32 = 18;
pub const PPHY_MISC2_CDR_BW_MASK: u32 = 0x3;
pub const PPHY_MISC2_CDR_BW_SHIFT: u32 = 16;

pub const PDLL_TRNS1: u32 = 0x1104;
pub const PDLL_TRNS1_D3PLLOFF_EN: u32 = 1 << 11;
pub const PDLL_TRNS1_REGCLK_SEL_NORM: u32 = 1 << 10;
pub const PDLL_TRNS1_REPLY_TO_MASK: u32 = 0x3FF;
pub const PDLL_TRNS1_REPLY_TO_SHIFT: u32 = 0;

pub const TLEXTN_STATS: u32 = 0x1208;
pub const TLEXTN_STATS_DEVNO_MASK: u32 = 0x1F;
pub const TLEXTN_STATS_DEVNO_SHIFT: u32 = 16;
pub const TLEXTN_STATS_BUSNO_MASK: u32 = 0xFF;
pub const TLEXTN_STATS_BUSNO_SHIFT: u32 = 8;

pub const EFUSE_CTRL: u32 = 0x12C0;
pub const EFUSE_CTRL_FLAG: u32 = 1 << 31;
pub const EUFSE_CTRL_ACK: u32 = 1 << 30;
pub const EFUSE_CTRL_ADDR_MASK: u32 = 0x3FF;
pub const EFUSE_CTRL_ADDR_SHIFT: u32 = 16;

pub const EFUSE_DATA: u32 = 0x12C4;

pub const SPI_OP1: u32 = 0x12C8;
pub const SPI_OP1_RDID_MASK: u32 = 0xFF;
pub const SPI_OP1_RDID_SHIFT: u32 = 24;
pub const SPI_OP1_CE_MASK: u32 = 0xFF;
pub const SPI_OP1_CE_SHIFT: u32 = 16;
pub const SPI_OP1_SE_MASK: u32 = 0xFF;
pub const SPI_OP1_SE_SHIFT: u32 = 8;
pub const SPI_OP1_PRGRM_MASK: u32 = 0xFF;
pub const SPI_OP1_PRGRM_SHIFT: u32 = 0;

pub const SPI_OP2: u32 = 0x12CC;
pub const SPI_OP2_READ_MASK: u32 = 0xFF;
pub const SPI_OP2_READ_SHIFT: u32 = 24;
pub const SPI_OP2_WRSR_MASK: u32 = 0xFF;
pub const SPI_OP2_WRSR_SHIFT: u32 = 16;
pub const SPI_OP2_RDSR_MASK: u32 = 0xFF;
pub const SPI_OP2_RDSR_SHIFT: u32 = 8;
pub const SPI_OP2_WREN_MASK: u32 = 0xFF;
pub const SPI_OP2_WREN_SHIFT: u32 = 0;

pub const SPI_OP3: u32 = 0x12E4;
pub const SPI_OP3_WRDI_MASK: u32 = 0xFF;
pub const SPI_OP3_WRDI_SHIFT: u32 = 8;
pub const SPI_OP3_EWSR_MASK: u32 = 0xFF;
pub const SPI_OP3_EWSR_SHIFT: u32 = 0;

pub const EF_CTRL: u32 = 0x12D0;
pub const EF_CTRL_FSTS_MASK: u32 = 0xFF;
pub const EF_CTRL_FSTS_SHIFT: u32 = 20;
pub const EF_CTRL_CLASS_MASK: u32 = 0x7;
pub const EF_CTRL_CLASS_SHIFT: u32 = 16;
pub const EF_CTRL_CLASS_F_UNKNOWN: u32 = 0;
pub const EF_CTRL_CLASS_F_STD: u32 = 1;
pub const EF_CTRL_CLASS_F_SST: u32 = 2;
pub const EF_CTRL_CLASS_E_UNKNOWN: u32 = 0;
pub const EF_CTRL_CLASS_E_1K: u32 = 1;
pub const EF_CTRL_CLASS_E_4K: u32 = 2;
pub const EF_CTRL_FRET: u32 = 1 << 15;
pub const EF_CTRL_TYP_MASK: u32 = 0x3;
pub const EF_CTRL_TYP_SHIFT: u32 = 12;
pub const EF_CTRL_TYP_NONE: u32 = 0;
pub const EF_CTRL_TYP_F: u32 = 1;
pub const EF_CTRL_TYP_E: u32 = 2;
pub const EF_CTRL_TYP_UNKNOWN: u32 = 3;
pub const EF_CTRL_ONE_CLK: u32 = 1 << 10;
pub const EF_CTRL_ECLK_MASK: u32 = 0x3;
pub const EF_CTRL_ECLK_SHIFT: u32 = 8;
pub const EF_CTRL_ECLK_125K: u32 = 0;
pub const EF_CTRL_ECLK_250K: u32 = 1;
pub const EF_CTRL_ECLK_500K: u32 = 2;
pub const EF_CTRL_ECLK_1M: u32 = 3;
pub const EF_CTRL_FBUSY: u32 = 1 << 7;
pub const EF_CTRL_ACTION: u32 = 1 << 6;
pub const EF_CTRL_AUTO_OP: u32 = 1 << 5;
pub const EF_CTRL_SST_MODE: u32 = 1 << 4;
pub const EF_CTRL_INST_MASK: u32 = 0xF;
pub const EF_CTRL_INST_SHIFT: u32 = 0;
pub const EF_CTRL_INST_NONE: u32 = 0;
pub const EF_CTRL_INST_READ: u32 = 1;
pub const EF_CTRL_INST_RDID: u32 = 2;
pub const EF_CTRL_INST_RDSR: u32 = 3;
pub const EF_CTRL_INST_WREN: u32 = 4;
pub const EF_CTRL_INST_PRGRM: u32 = 5;
pub const EF_CTRL_INST_SE: u32 = 6;
pub const EF_CTRL_INST_CE: u32 = 7;
pub const EF_CTRL_INST_WRSR: u32 = 10;
pub const EF_CTRL_INST_EWSR: u32 = 11;
pub const EF_CTRL_INST_WRDI: u32 = 12;
pub const EF_CTRL_INST_WRITE: u32 = 2;

pub const EF_ADDR: u32 = 0x12D4;
pub const EF_DATA: u32 = 0x12D8;
pub const SPI_ID: u32 = 0x12DC;

pub const SPI_CFG_START: u32 = 0x12E0;

pub const PMCTRL: u32 = 0x12F8;
pub const PMCTRL_HOTRST_WTEN: u32 = 1 << 31;
/* bit30: L0s/L1 controlled by MAC based on throughput(setting in: u32 = 15A0) */
pub const PMCTRL_ASPM_FCEN: u32 = 1 << 30;
pub const PMCTRL_SADLY_EN: u32 = 1 << 29;
pub const PMCTRL_L0S_BUFSRX_EN: u32 = 1 << 28;
pub const PMCTRL_LCKDET_TIMER_MASK: u32 = 0xF;
pub const PMCTRL_LCKDET_TIMER_SHIFT: u32 = 24;
pub const PMCTRL_LCKDET_TIMER_DEF: u32 = 0xC;
/* bit[23:20] if pm_request_l1 time > @, then enter L0s not L1 */
pub const PMCTRL_L1REQ_TO_MASK: u32 = 0xF;
pub const PMCTRL_L1REQ_TO_SHIFT: u32 = 20;
pub const PMCTRL_L1REG_TO_DEF: u32 = 0xF;
pub const PMCTRL_TXL1_AFTER_L0S: u32 = 1 << 19;
pub const PMCTRL_L1_TIMER_MASK: u32 = 0x7;
pub const PMCTRL_L1_TIMER_SHIFT: u32 = 16;
pub const PMCTRL_L1_TIMER_DIS: u32 = 0;
pub const PMCTRL_L1_TIMER_2US: u32 = 1;
pub const PMCTRL_L1_TIMER_4US: u32 = 2;
pub const PMCTRL_L1_TIMER_8US: u32 = 3;
pub const PMCTRL_L1_TIMER_16US: u32 = 4;
pub const PMCTRL_L1_TIMER_24US: u32 = 5;
pub const PMCTRL_L1_TIMER_32US: u32 = 6;
pub const PMCTRL_L1_TIMER_63US: u32 = 7;
pub const PMCTRL_RCVR_WT_1US: u32 = 1 << 15;
pub const PMCTRL_PWM_VER_11: u32 = 1 << 14;
/* bit13: enable pcie clk switch in L1 state */
pub const PMCTRL_L1_CLKSW_EN: u32 = 1 << 13;
pub const PMCTRL_L0S_EN: u32 = 1 << 12;
pub const PMCTRL_RXL1_AFTER_L0S: u32 = 1 << 11;
pub const PMCTRL_L0S_TIMER_MASK: u32 = 0x7;
pub const PMCTRL_L0S_TIMER_SHIFT: u32 = 8;
pub const PMCTRL_L1_BUFSRX_EN: u32 = 1 << 7;
/* bit6: power down serdes RX */
pub const PMCTRL_L1_SRDSRX_PWD: u32 = 1 << 6;
pub const PMCTRL_L1_SRDSPLL_EN: u32 = 1 << 5;
pub const PMCTRL_L1_SRDS_EN: u32 = 1 << 4;
pub const PMCTRL_L1_EN: u32 = 1 << 3;
pub const PMCTRL_CLKREQ_EN: u32 = 1 << 2;
pub const PMCTRL_RBER_EN: u32 = 1 << 1;
pub const PMCTRL_SPRSDWER_EN: u32 = 1 << 0;

pub const LTSSM_CTRL: u32 = 0x12FC;
pub const LTSSM_WRO_EN: u32 = 1 << 12;

/*******************************************************/
/* following registers are mapped only to memory space */
/*******************************************************/

pub const MASTER: u32 = 0x1400;
pub const MASTER_OTP_FLG: u32 = 1 << 31;
pub const MASTER_DEV_NUM_MASK: u32 = 0x7F;
pub const MASTER_DEV_NUM_SHIFT: u32 = 24;
pub const MASTER_REV_NUM_MASK: u32 = 0xFF;
pub const MASTER_REV_NUM_SHIFT: u32 = 16;
pub const MASTER_DEASSRT: u32 = 1 << 15;
pub const MASTER_RDCLR_INT: u32 = 1 << 14;
pub const MASTER_DMA_RST: u32 = 1 << 13;
/* bit12:: u32 = 1:alwys select pclk from serdes, not sw to: u32 = 25M */
pub const MASTER_PCLKSEL_SRDS: u32 = 1 << 12;
/* bit11: irq moduration for rx */
pub const MASTER_IRQMOD2_EN: u32 = 1 << 11;
/* bit10: irq moduration for tx/rx */
pub const MASTER_IRQMOD1_EN: u32 = 1 << 10;
pub const MASTER_MANU_INT: u32 = 1 << 9;
pub const MASTER_MANUTIMER_EN: u32 = 1 << 8;
pub const MASTER_SYSALVTIMER_EN: u32 = 1 << 7;
pub const MASTER_OOB_DIS: u32 = 1 << 6;
/* bit5: wakeup without pcie clk */
pub const MASTER_WAKEN_25M: u32 = 1 << 5;
pub const MASTER_BERT_START: u32 = 1 << 4;
pub const MASTER_PCIE_TSTMOD_MASK: u32 = 0x3;
pub const MASTER_PCIE_TSTMOD_SHIFT: u32 = 2;
pub const MASTER_PCIE_RST: u32 = 1 << 1;
/* bit0: MAC & DMA reset */
pub const MASTER_DMA_MAC_RST: u32 = 1 << 0;
pub const DMA_MAC_RST_TO: u32 = 50;

pub const MANU_TIMER: u32 = 0x1404;

pub const IRQ_MODU_TIMER: u32 = 0x1408;
/* hi-16bit is only for RX */
pub const IRQ_MODU_TIMER2_MASK: u32 = 0xFFFF;
pub const IRQ_MODU_TIMER2_SHIFT: u32 = 16;
pub const IRQ_MODU_TIMER1_MASK: u32 = 0xFFFF;
pub const IRQ_MODU_TIMER1_SHIFT: u32 = 0;

pub const PHY_CTRL: u32 = 0x140C;
pub const PHY_CTRL_ADDR_MASK: u32 = 0x1F;
pub const PHY_CTRL_ADDR_SHIFT: u32 = 19;
pub const PHY_CTRL_BP_VLTGSW: u32 = 1 << 18;
pub const PHY_CTRL_100AB_EN: u32 = 1 << 17;
pub const PHY_CTRL_10AB_EN: u32 = 1 << 16;
pub const PHY_CTRL_PLL_BYPASS: u32 = 1 << 15;
/* bit14: affect MAC & PHY, go to low power sts */
pub const PHY_CTRL_POWER_DOWN: u32 = 1 << 14;
/* bit13:: u32 = 1:pll always ON,: u32 = 0:can switch in lpw */
pub const PHY_CTRL_PLL_ON: u32 = 1 << 13;
pub const PHY_CTRL_RST_ANALOG: u32 = 1 << 12;
pub const PHY_CTRL_HIB_PULSE: u32 = 1 << 11;
pub const PHY_CTRL_HIB_EN: u32 = 1 << 10;
pub const PHY_CTRL_GIGA_DIS: u32 = 1 << 9;
/* bit8: poweron rst */
pub const PHY_CTRL_IDDQ_DIS: u32 = 1 << 8;
/* bit7: while reboot, it affects bit8 */
pub const PHY_CTRL_IDDQ: u32 = 1 << 7;
pub const PHY_CTRL_LPW_EXIT: u32 = 1 << 6;
pub const PHY_CTRL_GATE_25M: u32 = 1 << 5;
pub const PHY_CTRL_RVRS_ANEG: u32 = 1 << 4;
pub const PHY_CTRL_ANEG_NOW: u32 = 1 << 3;
pub const PHY_CTRL_LED_MODE: u32 = 1 << 2;
pub const PHY_CTRL_RTL_MODE: u32 = 1 << 1;
/* bit0: out of dsp RST state */
pub const PHY_CTRL_DSPRST_OUT: u32 = 1 << 0;
pub const PHY_CTRL_DSPRST_TO: u32 = 80;
pub const PHY_CTRL_CLS: u32 = PHY_CTRL_LED_MODE | PHY_CTRL_100AB_EN | PHY_CTRL_PLL_ON;

pub const MAC_STS: u32 = 0x1410;
pub const MAC_STS_SFORCE_MASK: u32 = 0xF;
pub const MAC_STS_SFORCE_SHIFT: u32 = 14;
pub const MAC_STS_CALIB_DONE: u32 = 1 << 13;
pub const MAC_STS_CALIB_RES_MASK: u32 = 0x1F;
pub const MAC_STS_CALIB_RES_SHIFT: u32 = 8;
pub const MAC_STS_CALIBERR_MASK: u32 = 0xF;
pub const MAC_STS_CALIBERR_SHIFT: u32 = 4;
pub const MAC_STS_TXQ_BUSY: u32 = 1 << 3;
pub const MAC_STS_RXQ_BUSY: u32 = 1 << 2;
pub const MAC_STS_TXMAC_BUSY: u32 = 1 << 1;
pub const MAC_STS_RXMAC_BUSY: u32 = 1 << 0;
pub const MAC_STS_IDLE: u32 =
    MAC_STS_TXQ_BUSY | MAC_STS_RXQ_BUSY | MAC_STS_TXMAC_BUSY | MAC_STS_RXMAC_BUSY;

pub const MDIO: u32 = 0x1414;
pub const MDIO_MODE_EXT: u32 = 1 << 30;
pub const MDIO_POST_READ: u32 = 1 << 29;
pub const MDIO_AUTO_POLLING: u32 = 1 << 28;
pub const MDIO_BUSY: u32 = 1 << 27;
pub const MDIO_CLK_SEL_MASK: u32 = 0x7;
pub const MDIO_CLK_SEL_SHIFT: u32 = 24;
pub const MDIO_CLK_SEL_25MD4: u16 = 0;
pub const MDIO_CLK_SEL_25MD6: u16 = 2;
pub const MDIO_CLK_SEL_25MD8: u16 = 3;
pub const MDIO_CLK_SEL_25MD10: u16 = 4;
pub const MDIO_CLK_SEL_25MD32: u16 = 5;
pub const MDIO_CLK_SEL_25MD64: u16 = 6;
pub const MDIO_CLK_SEL_25MD128: u16 = 7;
pub const MDIO_START: u32 = 1 << 23;
pub const MDIO_SPRES_PRMBL: u32 = 1 << 22;
/* bit21:: u32 = 1:read,0:write */
pub const MDIO_OP_READ: u32 = 1 << 21;
pub const MDIO_REG_MASK: u32 = 0x1F;
pub const MDIO_REG_SHIFT: u32 = 16;
pub const MDIO_DATA_MASK: u32 = 0xFFFF;
pub const MDIO_DATA_SHIFT: u32 = 0;
pub const MDIO_MAX_AC_TO: u32 = 120;

pub const MDIO_EXTN: u32 = 0x1448;
pub const MDIO_EXTN_PORTAD_MASK: u32 = 0x1F;
pub const MDIO_EXTN_PORTAD_SHIFT: u32 = 21;
pub const MDIO_EXTN_DEVAD_MASK: u32 = 0x1F;
pub const MDIO_EXTN_DEVAD_SHIFT: u32 = 16;
pub const MDIO_EXTN_REG_MASK: u32 = 0xFFFF;
pub const MDIO_EXTN_REG_SHIFT: u32 = 0;

pub const PHY_STS: u32 = 0x1418;
pub const PHY_STS_LPW: u32 = 1 << 31;
pub const PHY_STS_LPI: u32 = 1 << 30;
pub const PHY_STS_PWON_STRIP_MASK: u32 = 0xFFF;
pub const PHY_STS_PWON_STRIP_SHIFT: u32 = 16;

pub const PHY_STS_DUPLEX: u32 = 1 << 3;
pub const PHY_STS_LINKUP: u32 = 1 << 2;
pub const PHY_STS_SPEED_MASK: u32 = 0x3;
pub const PHY_STS_SPEED_SHIFT: u32 = 0;
pub const PHY_STS_SPEED_1000M: u32 = 2;
pub const PHY_STS_SPEED_100M: u32 = 1;
pub const PHY_STS_SPEED_10M: u32 = 0;

pub const BIST0: u32 = 0x141C;
pub const BIST0_COL_MASK: u32 = 0x3F;
pub const BIST0_COL_SHIFT: u32 = 24;
pub const BIST0_ROW_MASK: u32 = 0xFFF;
pub const BIST0_ROW_SHIFT: u32 = 12;
pub const BIST0_STEP_MASK: u32 = 0xF;
pub const BIST0_STEP_SHIFT: u32 = 8;
pub const BIST0_PATTERN_MASK: u32 = 0x7;
pub const BIST0_PATTERN_SHIFT: u32 = 4;
pub const BIST0_CRIT: u32 = 1 << 3;
pub const BIST0_FIXED: u32 = 1 << 2;
pub const BIST0_FAIL: u32 = 1 << 1;
pub const BIST0_START: u32 = 1 << 0;

pub const BIST1: u32 = 0x1420;
pub const BIST1_COL_MASK: u32 = 0x3F;
pub const BIST1_COL_SHIFT: u32 = 24;
pub const BIST1_ROW_MASK: u32 = 0xFFF;
pub const BIST1_ROW_SHIFT: u32 = 12;
pub const BIST1_STEP_MASK: u32 = 0xF;
pub const BIST1_STEP_SHIFT: u32 = 8;
pub const BIST1_PATTERN_MASK: u32 = 0x7;
pub const BIST1_PATTERN_SHIFT: u32 = 4;
pub const BIST1_CRIT: u32 = 1 << 3;
pub const BIST1_FIXED: u32 = 1 << 2;
pub const BIST1_FAIL: u32 = 1 << 1;
pub const BIST1_START: u32 = 1 << 0;

pub const SERDES: u32 = 0x1424;
pub const SERDES_PHYCLK_SLWDWN: u32 = 1 << 18;
pub const SERDES_MACCLK_SLWDWN: u32 = 1 << 17;
pub const SERDES_SELFB_PLL_MASK: u32 = 0x3;
pub const SERDES_SELFB_PLL_SHIFT: u32 = 14;
/* bit13:: u32 = 1:gtx_clk,: u32 = 0:25M */
pub const SERDES_PHYCLK_SEL_GTX: u32 = 1 << 13;
/* bit12:: u32 = 1:serdes,0:25M */
pub const SERDES_PCIECLK_SEL_SRDS: u32 = 1 << 12;
pub const SERDES_BUFS_RX_EN: u32 = 1 << 11;
pub const SERDES_PD_RX: u32 = 1 << 10;
pub const SERDES_PLL_EN: u32 = 1 << 9;
pub const SERDES_EN: u32 = 1 << 8;
/* bit6:: u32 = 0:state-machine,1:csr */
pub const SERDES_SELFB_PLL_SEL_CSR: u32 = 1 << 6;
pub const SERDES_SELFB_PLL_CSR_MASK: u32 = 0x3;
pub const SERDES_SELFB_PLL_CSR_SHIFT: u32 = 4;
/*: u32 = 4-12% OV-CLK */
pub const SERDES_SELFB_PLL_CSR_4: u32 = 3;
/*: u32 = 0-4% OV-CLK */
pub const SERDES_SELFB_PLL_CSR_0: u32 = 2;
/*: u32 = 12-18% OV-CLK */
pub const SERDES_SELFB_PLL_CSR_12: u32 = 1;
/*: u32 = 18-25% OV-CLK */
pub const SERDES_SELFB_PLL_CSR_18: u32 = 0;
pub const SERDES_VCO_SLOW: u32 = 1 << 3;
pub const SERDES_VCO_FAST: u32 = 1 << 2;
pub const SERDES_LOCKDCT_EN: u32 = 1 << 1;
pub const SERDES_LOCKDCTED: u32 = 1 << 0;

pub const LED_CTRL: u32 = 0x1428;
pub const LED_CTRL_PATMAP2_MASK: u32 = 0x3;
pub const LED_CTRL_PATMAP2_SHIFT: u32 = 8;
pub const LED_CTRL_PATMAP1_MASK: u32 = 0x3;
pub const LED_CTRL_PATMAP1_SHIFT: u32 = 6;
pub const LED_CTRL_PATMAP0_MASK: u32 = 0x3;
pub const LED_CTRL_PATMAP0_SHIFT: u32 = 4;
pub const LED_CTRL_D3_MODE_MASK: u32 = 0x3;
pub const LED_CTRL_D3_MODE_SHIFT: u32 = 2;
pub const LED_CTRL_D3_MODE_NORMAL: u32 = 0;
pub const LED_CTRL_D3_MODE_WOL_DIS: u32 = 1;
pub const LED_CTRL_D3_MODE_WOL_ANY: u32 = 2;
pub const LED_CTRL_D3_MODE_WOL_EN: u32 = 3;
pub const LED_CTRL_DUTY_CYCL_MASK: u32 = 0x3;
pub const LED_CTRL_DUTY_CYCL_SHIFT: u32 = 0;
/*: u32 = 50% */
pub const LED_CTRL_DUTY_CYCL_50: u32 = 0;
/*: u32 = 12.5% */
pub const LED_CTRL_DUTY_CYCL_125: u32 = 1;
/*: u32 = 25% */
pub const LED_CTRL_DUTY_CYCL_25: u32 = 2;
/*: u32 = 75% */
pub const LED_CTRL_DUTY_CYCL_75: u32 = 3;

pub const LED_PATN: u32 = 0x142C;
pub const LED_PATN1_MASK: u32 = 0xFFFF;
pub const LED_PATN1_SHIFT: u32 = 16;
pub const LED_PATN0_MASK: u32 = 0xFFFF;
pub const LED_PATN0_SHIFT: u32 = 0;

pub const LED_PATN2: u32 = 0x1430;
pub const LED_PATN2_MASK: u32 = 0xFFFF;
pub const LED_PATN2_SHIFT: u32 = 0;

pub const SYSALV: u32 = 0x1434;
pub const SYSALV_FLAG: u32 = 1 << 0;

pub const PCIERR_INST: u32 = 0x1438;
pub const PCIERR_INST_TX_RATE_MASK: u32 = 0xF;
pub const PCIERR_INST_TX_RATE_SHIFT: u32 = 4;
pub const PCIERR_INST_RX_RATE_MASK: u32 = 0xF;
pub const PCIERR_INST_RX_RATE_SHIFT: u32 = 0;

pub const LPI_DECISN_TIMER: u32 = 0x143C;

pub const LPI_CTRL: u32 = 0x1440;
pub const LPI_CTRL_CHK_DA: u32 = 1 << 31;
pub const LPI_CTRL_ENH_TO_MASK: u32 = 0x1FFF;
pub const LPI_CTRL_ENH_TO_SHIFT: u32 = 12;
pub const LPI_CTRL_ENH_TH_MASK: u32 = 0x1F;
pub const LPI_CTRL_ENH_TH_SHIFT: u32 = 6;
pub const LPI_CTRL_ENH_EN: u32 = 1 << 5;
pub const LPI_CTRL_CHK_RX: u32 = 1 << 4;
pub const LPI_CTRL_CHK_STATE: u32 = 1 << 3;
pub const LPI_CTRL_GMII: u32 = 1 << 2;
pub const LPI_CTRL_TO_PHY: u32 = 1 << 1;
pub const LPI_CTRL_EN: u32 = 1 << 0;

pub const LPI_WAIT: u32 = 0x1444;
pub const LPI_WAIT_TIMER_MASK: u32 = 0xFFFF;
pub const LPI_WAIT_TIMER_SHIFT: u32 = 0;

/* heart-beat, for swoi/cifs */
pub const HRTBT_VLAN: u32 = 0x1450;
pub const HRTBT_VLANID_MASK: u32 = 0xFFFF;
pub const HRRBT_VLANID_SHIFT: u32 = 0;

pub const HRTBT_CTRL: u32 = 0x1454;
pub const HRTBT_CTRL_EN: u32 = 1 << 31;
pub const HRTBT_CTRL_PERIOD_MASK: u32 = 0x3F;
pub const HRTBT_CTRL_PERIOD_SHIFT: u32 = 25;
pub const HRTBT_CTRL_HASVLAN: u32 = 1 << 24;
pub const HRTBT_CTRL_HDRADDR_MASK: u32 = 0xFFF;
pub const HRTBT_CTRL_HDRADDR_SHIFT: u32 = 12;
pub const HRTBT_CTRL_HDRADDRB0_MASK: u32 = 0x7FF;
pub const HRTBT_CTRL_HDRADDRB0_SHIFT: u32 = 13;
pub const HRTBT_CTRL_PKT_FRAG: u32 = 1 << 12;
pub const HRTBT_CTRL_PKTLEN_MASK: u32 = 0xFFF;
pub const HRTBT_CTRL_PKTLEN_SHIFT: u32 = 0;

/* for: u32 = B0+, bit[13..] for C0+ */
pub const HRTBT_EXT_CTRL: u32 = 0x1AD0;
pub const L1F_HRTBT_EXT_CTRL_PERIOD_HIGH_MASK: u32 = 0x3F;
pub const L1F_HRTBT_EXT_CTRL_PERIOD_HIGH_SHIFT: u32 = 24;
pub const L1F_HRTBT_EXT_CTRL_SWOI_STARTUP_PKT_EN: u32 = 1 << 23;
pub const L1F_HRTBT_EXT_CTRL_IOAC_2_FRAGMENTED: u32 = 1 << 22;
pub const L1F_HRTBT_EXT_CTRL_IOAC_1_FRAGMENTED: u32 = 1 << 21;
pub const L1F_HRTBT_EXT_CTRL_IOAC_1_KEEPALIVE_EN: u32 = 1 << 20;
pub const L1F_HRTBT_EXT_CTRL_IOAC_1_HAS_VLAN: u32 = 1 << 19;
pub const L1F_HRTBT_EXT_CTRL_IOAC_1_IS_8023: u32 = 1 << 18;
pub const L1F_HRTBT_EXT_CTRL_IOAC_1_IS_IPV6: u32 = 1 << 17;
pub const L1F_HRTBT_EXT_CTRL_IOAC_2_KEEPALIVE_EN: u32 = 1 << 16;
pub const L1F_HRTBT_EXT_CTRL_IOAC_2_HAS_VLAN: u32 = 1 << 15;
pub const L1F_HRTBT_EXT_CTRL_IOAC_2_IS_8023: u32 = 1 << 14;
pub const L1F_HRTBT_EXT_CTRL_IOAC_2_IS_IPV6: u32 = 1 << 13;
pub const HRTBT_EXT_CTRL_NS_EN: u32 = 1 << 12;
pub const HRTBT_EXT_CTRL_FRAG_LEN_MASK: u32 = 0xFF;
pub const HRTBT_EXT_CTRL_FRAG_LEN_SHIFT: u32 = 4;
pub const HRTBT_EXT_CTRL_IS_8023: u32 = 1 << 3;
pub const HRTBT_EXT_CTRL_IS_IPV6: u32 = 1 << 2;
pub const HRTBT_EXT_CTRL_WAKEUP_EN: u32 = 1 << 1;
pub const HRTBT_EXT_CTRL_ARP_EN: u32 = 1 << 0;

pub const HRTBT_REM_IPV4_ADDR: u32 = 0x1AD4;
pub const HRTBT_HOST_IPV4_ADDR: u32 = 0x1478;
pub const HRTBT_REM_IPV6_ADDR3: u32 = 0x1AD8;
pub const HRTBT_REM_IPV6_ADDR2: u32 = 0x1ADC;
pub const HRTBT_REM_IPV6_ADDR1: u32 = 0x1AE0;
pub const HRTBT_REM_IPV6_ADDR0: u32 = 0x1AE4;

/*: u32 = 1B8C ~: u32 = 1B94 for C0+ */
pub const SWOI_ACER_CTRL: u32 = 0x1B8C;
pub const SWOI_ORIG_ACK_NAK_EN: u32 = 1 << 20;
pub const SWOI_ORIG_ACK_NAK_PKT_LEN_MASK: u32 = 0xFF;
pub const SWOI_ORIG_ACK_NAK_PKT_LEN_SHIFT: u32 = 12;
pub const SWOI_ORIG_ACK_ADDR_MASK: u32 = 0xFFF;
pub const SWOI_ORIG_ACK_ADDR_SHIFT: u32 = 0;

pub const SWOI_IOAC_CTRL_2: u32 = 0x1B90;
pub const SWOI_IOAC_CTRL_2_SWOI_1_FRAG_LEN_MASK: u32 = 0xFF;
pub const SWOI_IOAC_CTRL_2_SWOI_1_FRAG_LEN_SHIFT: u32 = 24;
pub const SWOI_IOAC_CTRL_2_SWOI_1_PKT_LEN_MASK: u32 = 0xFFF;
pub const SWOI_IOAC_CTRL_2_SWOI_1_PKT_LEN_SHIFT: u32 = 12;
pub const SWOI_IOAC_CTRL_2_SWOI_1_HDR_ADDR_MASK: u32 = 0xFFF;
pub const SWOI_IOAC_CTRL_2_SWOI_1_HDR_ADDR_SHIFT: u32 = 0;

pub const SWOI_IOAC_CTRL_3: u32 = 0x1B94;
pub const SWOI_IOAC_CTRL_3_SWOI_2_FRAG_LEN_MASK: u32 = 0xFF;
pub const SWOI_IOAC_CTRL_3_SWOI_2_FRAG_LEN_SHIFT: u32 = 24;
pub const SWOI_IOAC_CTRL_3_SWOI_2_PKT_LEN_MASK: u32 = 0xFFF;
pub const SWOI_IOAC_CTRL_3_SWOI_2_PKT_LEN_SHIFT: u32 = 12;
pub const SWOI_IOAC_CTRL_3_SWOI_2_HDR_ADDR_MASK: u32 = 0xFFF;
pub const SWOI_IOAC_CTRL_3_SWOI_2_HDR_ADDR_SHIFT: u32 = 0;

/*SWOI_HOST_IPV6_ADDR reuse reg1a60-1a6c,: u32 = 1a70-1a7c,: u32 = 1aa0-1aac,: u32 = 1ab0-1abc.*/
pub const HRTBT_WAKEUP_PORT: u32 = 0x1AE8;
pub const HRTBT_WAKEUP_PORT_SRC_MASK: u32 = 0xFFFF;
pub const HRTBT_WAKEUP_PORT_SRC_SHIFT: u32 = 16;
pub const HRTBT_WAKEUP_PORT_DEST_MASK: u32 = 0xFFFF;
pub const HRTBT_WAKEUP_PORT_DEST_SHIFT: u32 = 0;

pub const HRTBT_WAKEUP_DATA7: u32 = 0x1AEC;
pub const HRTBT_WAKEUP_DATA6: u32 = 0x1AF0;
pub const HRTBT_WAKEUP_DATA5: u32 = 0x1AF4;
pub const HRTBT_WAKEUP_DATA4: u32 = 0x1AF8;
pub const HRTBT_WAKEUP_DATA3: u32 = 0x1AFC;
pub const HRTBT_WAKEUP_DATA2: u32 = 0x1B80;
pub const HRTBT_WAKEUP_DATA1: u32 = 0x1B84;
pub const HRTBT_WAKEUP_DATA0: u32 = 0x1B88;

pub const RXPARSE: u32 = 0x1458;
pub const RXPARSE_FLT6_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT6_L4_SHIFT: u32 = 30;
pub const RXPARSE_FLT6_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT6_L3_SHIFT: u32 = 28;
pub const RXPARSE_FLT5_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT5_L4_SHIFT: u32 = 26;
pub const RXPARSE_FLT5_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT5_L3_SHIFT: u32 = 24;
pub const RXPARSE_FLT4_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT4_L4_SHIFT: u32 = 22;
pub const RXPARSE_FLT4_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT4_L3_SHIFT: u32 = 20;
pub const RXPARSE_FLT3_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT3_L4_SHIFT: u32 = 18;
pub const RXPARSE_FLT3_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT3_L3_SHIFT: u32 = 16;
pub const RXPARSE_FLT2_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT2_L4_SHIFT: u32 = 14;
pub const RXPARSE_FLT2_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT2_L3_SHIFT: u32 = 12;
pub const RXPARSE_FLT1_L4_MASK: u32 = 0x3;
pub const RXPARSE_FLT1_L4_SHIFT: u32 = 10;
pub const RXPARSE_FLT1_L3_MASK: u32 = 0x3;
pub const RXPARSE_FLT1_L3_SHIFT: u32 = 8;
pub const RXPARSE_FLT6_EN: u32 = 1 << 5;
pub const RXPARSE_FLT5_EN: u32 = 1 << 4;
pub const RXPARSE_FLT4_EN: u32 = 1 << 3;
pub const RXPARSE_FLT3_EN: u32 = 1 << 2;
pub const RXPARSE_FLT2_EN: u32 = 1 << 1;
pub const RXPARSE_FLT1_EN: u32 = 1 << 0;
pub const RXPARSE_FLT_L4_UDP: u32 = 0;
pub const RXPARSE_FLT_L4_TCP: u32 = 1;
pub const RXPARSE_FLT_L4_BOTH: u32 = 2;
pub const RXPARSE_FLT_L4_NONE: u32 = 3;
pub const RXPARSE_FLT_L3_IPV6: u32 = 0;
pub const RXPARSE_FLT_L3_IPV4: u32 = 1;
pub const RXPARSE_FLT_L3_BOTH: u32 = 2;

/* Terodo support */
pub const TRD_CTRL: u32 = 0x145C;
pub const TRD_CTRL_EN: u32 = 1 << 31;
pub const TRD_CTRL_BUBBLE_WAKE_EN: u32 = 1 << 30;
pub const TRD_CTRL_PREFIX_CMP_HW: u32 = 1 << 28;
pub const TRD_CTRL_RSHDR_ADDR_MASK: u32 = 0xFFF;
pub const TRD_CTRL_RSHDR_ADDR_SHIFT: u32 = 16;
pub const TRD_CTRL_SINTV_MAX_MASK: u32 = 0xFF;
pub const TRD_CTRL_SINTV_MAX_SHIFT: u32 = 8;
pub const TRD_CTRL_SINTV_MIN_MASK: u32 = 0xFF;
pub const TRD_CTRL_SINTV_MIN_SHIFT: u32 = 0;

pub const TRD_RS: u32 = 0x1460;
pub const TRD_RS_SZ_MASK: u32 = 0xFFF;
pub const TRD_RS_SZ_SHIFT: u32 = 20;
pub const TRD_RS_NONCE_OFS_MASK: u32 = 0xFFF;
pub const TRD_RS_NONCE_OFS_SHIFT: u32 = 8;
pub const TRD_RS_SEQ_OFS_MASK: u32 = 0xFF;
pub const TRD_RS_SEQ_OFS_SHIFT: u32 = 0;

pub const TRD_SRV_IP4: u32 = 0x1464;

pub const TRD_CLNT_EXTNL_IP4: u32 = 0x1468;

pub const TRD_PORT: u32 = 0x146C;
pub const TRD_PORT_CLNT_EXTNL_MASK: u32 = 0xFFFF;
pub const TRD_PORT_CLNT_EXTNL_SHIFT: u32 = 16;
pub const TRD_PORT_SRV_MASK: u32 = 0xFFFF;
pub const TRD_PORT_SRV_SHIFT: u32 = 0;

pub const TRD_PREFIX: u32 = 0x1470;

pub const TRD_BUBBLE_DA_IP4: u32 = 0x1478;

pub const TRD_BUBBLE_DA_PORT: u32 = 0x147C;

/* for: u32 = B0 */
pub const IDLE_DECISN_TIMER: u32 = 0x1474;
/*: u32 = 1ms */
pub const IDLE_DECISN_TIMER_DEF: u32 = 0x400;

pub const MAC_CTRL: u32 = 0x1480;
pub const MAC_CTRL_FAST_PAUSE: u32 = 1 << 31;
pub const MAC_CTRL_WOLSPED_SWEN: u32 = 1 << 30;
/* bit29:: u32 = 1:legacy(hi5b),: u32 = 0:marvl(lo5b)*/
pub const MAC_CTRL_MHASH_ALG_HI5B: u32 = 1 << 29;
pub const MAC_CTRL_SPAUSE_EN: u32 = 1 << 28;
pub const MAC_CTRL_DBG_EN: u32 = 1 << 27;
pub const MAC_CTRL_BRD_EN: u32 = 1 << 26;
pub const MAC_CTRL_MULTIALL_EN: u32 = 1 << 25;
pub const MAC_CTRL_RX_XSUM_EN: u32 = 1 << 24;
pub const MAC_CTRL_THUGE: u32 = 1 << 23;
pub const MAC_CTRL_MBOF: u32 = 1 << 22;
pub const MAC_CTRL_SPEED_MASK: u32 = 0x3;
pub const MAC_CTRL_SPEED_SHIFT: u32 = 20;
pub const MAC_CTRL_SPEED_10_100: u32 = 1;
pub const MAC_CTRL_SPEED_1000: u32 = 2;
pub const MAC_CTRL_SIMR: u32 = 1 << 19;
pub const MAC_CTRL_SSTCT: u32 = 1 << 17;
pub const MAC_CTRL_TPAUSE: u32 = 1 << 16;
pub const MAC_CTRL_PROMISC_EN: u32 = 1 << 15;
pub const MAC_CTRL_VLANSTRIP: u32 = 1 << 14;
pub const MAC_CTRL_PRMBLEN_MASK: u32 = 0xF;
pub const MAC_CTRL_PRMBLEN_SHIFT: u32 = 10;
pub const MAC_CTRL_RHUGE_EN: u32 = 1 << 9;
pub const MAC_CTRL_FLCHK: u32 = 1 << 8;
pub const MAC_CTRL_PCRCE: u32 = 1 << 7;
pub const MAC_CTRL_CRCE: u32 = 1 << 6;
pub const MAC_CTRL_FULLD: u32 = 1 << 5;
pub const MAC_CTRL_LPBACK_EN: u32 = 1 << 4;
pub const MAC_CTRL_RXFC_EN: u32 = 1 << 3;
pub const MAC_CTRL_TXFC_EN: u32 = 1 << 2;
pub const MAC_CTRL_RX_EN: u32 = 1 << 1;
pub const MAC_CTRL_TX_EN: u32 = 1 << 0;

pub const GAP: u32 = 0x1484;
pub const GAP_IPGR2_MASK: u32 = 0x7F;
pub const GAP_IPGR2_SHIFT: u32 = 24;
pub const GAP_IPGR1_MASK: u32 = 0x7F;
pub const GAP_IPGR1_SHIFT: u32 = 16;
pub const GAP_MIN_IFG_MASK: u32 = 0xFF;
pub const GAP_MIN_IFG_SHIFT: u32 = 8;
pub const GAP_IPGT_MASK: u32 = 0x7F;
pub const GAP_IPGT_SHIFT: u32 = 0;

pub const STAD0: u32 = 0x1488;
pub const STAD1: u32 = 0x148C;

pub const HASH_TBL0: u32 = 0x1490;
pub const HASH_TBL1: u32 = 0x1494;

pub const HALFD: u32 = 0x1498;
pub const HALFD_JAM_IPG_MASK: u32 = 0xF;
pub const HALFD_JAM_IPG_SHIFT: u32 = 24;
pub const HALFD_ABEBT_MASK: u32 = 0xF;
pub const HALFD_ABEBT_SHIFT: u32 = 20;
pub const HALFD_ABEBE: u32 = 1 << 19;
pub const HALFD_BPNB: u32 = 1 << 18;
pub const HALFD_NOBO: u32 = 1 << 17;
pub const HALFD_EDXSDFR: u32 = 1 << 16;
pub const HALFD_RETRY_MASK: u32 = 0xF;
pub const HALFD_RETRY_SHIFT: u32 = 12;
pub const HALFD_LCOL_MASK: u32 = 0x3FF;
pub const HALFD_LCOL_SHIFT: u32 = 0;

pub const MTU: u32 = 0x149C;
pub const MTU_JUMBO_TH: u32 = 1514;
pub const MTU_STD_ALGN: u32 = 1536;
pub const MTU_MIN: u32 = 64;

pub const SRAM0: u32 = 0x1500;
pub const SRAM_RFD_TAIL_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_RFD_TAIL_ADDR_SHIFT: u32 = 16;
pub const SRAM_RFD_HEAD_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_RFD_HEAD_ADDR_SHIFT: u32 = 0;

pub const SRAM1: u32 = 0x1510;
pub const SRAM_RFD_LEN_MASK: u32 = 0xFFF;
pub const SRAM_RFD_LEN_SHIFT: u32 = 0;

pub const SRAM2: u32 = 0x1518;
pub const SRAM_TRD_TAIL_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_TRD_TAIL_ADDR_SHIFT: u32 = 16;
pub const SRMA_TRD_HEAD_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_TRD_HEAD_ADDR_SHIFT: u32 = 0;

pub const SRAM3: u32 = 0x151C;
pub const SRAM_TRD_LEN_MASK: u32 = 0xFFF;
pub const SRAM_TRD_LEN_SHIFT: u32 = 0;

pub const SRAM4: u32 = 0x1520;
pub const SRAM_RXF_TAIL_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_RXF_TAIL_ADDR_SHIFT: u32 = 16;
pub const SRAM_RXF_HEAD_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_RXF_HEAD_ADDR_SHIFT: u32 = 0;

pub const SRAM5: u32 = 0x1524;
pub const SRAM_RXF_LEN_MASK: u32 = 0xFFF;
pub const SRAM_RXF_LEN_SHIFT: u32 = 0;
pub const SRAM_RXF_LEN_8K: u32 = (8 * 1024);

pub const SRAM6: u32 = 0x1528;
pub const SRAM_TXF_TAIL_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_TXF_TAIL_ADDR_SHIFT: u32 = 16;
pub const SRAM_TXF_HEAD_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_TXF_HEAD_ADDR_SHIFT: u32 = 0;

pub const SRAM7: u32 = 0x152C;
pub const SRAM_TXF_LEN_MASK: u32 = 0xFFF;
pub const SRAM_TXF_LEN_SHIFT: u32 = 0;

pub const SRAM8: u32 = 0x1530;
pub const SRAM_PATTERN_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_PATTERN_ADDR_SHIFT: u32 = 16;
pub const SRAM_TSO_ADDR_MASK: u32 = 0xFFF;
pub const SRAM_TSO_ADDR_SHIFT: u32 = 0;

pub const SRAM9: u32 = 0x1534;
pub const SRAM_LOAD_PTR: u32 = 1 << 0;

pub const RX_BASE_ADDR_HI: u32 = 0x1540;

pub const TX_BASE_ADDR_HI: u32 = 0x1544;

pub const RFD_ADDR_LO: u32 = 0x1550;
pub const RFD_RING_SZ: u32 = 0x1560;
pub const RFD_BUF_SZ: u32 = 0x1564;
pub const RFD_BUF_SZ_MASK: u32 = 0xFFFF;
pub const RFD_BUF_SZ_SHIFT: u32 = 0;

pub const RRD_ADDR_LO: u32 = 0x1568;
pub const RRD_RING_SZ: u32 = 0x1578;
pub const RRD_RING_SZ_MASK: u32 = 0xFFF;
pub const RRD_RING_SZ_SHIFT: u32 = 0;

/* pri3: highest, pri0: lowest */
pub const TPD_PRI3_ADDR_LO: u32 = 0x14E4;
pub const TPD_PRI2_ADDR_LO: u32 = 0x14E0;
pub const TPD_PRI1_ADDR_LO: u32 = 0x157C;
pub const TPD_PRI0_ADDR_LO: u32 = 0x1580;

/* producer index is: u32 = 16bit */
pub const TPD_PRI3_PIDX: u32 = 0x1618;
pub const TPD_PRI2_PIDX: u32 = 0x161A;
pub const TPD_PRI1_PIDX: u32 = 0x15F0;
pub const TPD_PRI0_PIDX: u32 = 0x15F2;

/* consumer index is: u32 = 16bit */
pub const TPD_PRI3_CIDX: u32 = 0x161C;
pub const TPD_PRI2_CIDX: u32 = 0x161E;
pub const TPD_PRI1_CIDX: u32 = 0x15F4;
pub const TPD_PRI0_CIDX: u32 = 0x15F6;

pub const TPD_RING_SZ: u32 = 0x1584;
pub const TPD_RING_SZ_MASK: u32 = 0xFFFF;
pub const TPD_RING_SZ_SHIFT: u32 = 0;

pub const CMB_ADDR_LO: u32 = 0x1588;

pub const TXQ0: u32 = 0x1590;
pub const TXQ0_TXF_BURST_PREF_MASK: u32 = 0xFFFF;
pub const TXQ0_TXF_BURST_PREF_SHIFT: u32 = 16;
pub const TXQ_TXF_BURST_PREF_DEF: u32 = 0x200;
pub const TXQ0_PEDING_CLR: u32 = 1 << 8;
pub const TXQ0_LSO_8023_EN: u32 = 1 << 7;
pub const TXQ0_MODE_ENHANCE: u32 = 1 << 6;
pub const TXQ0_EN: u32 = 1 << 5;
pub const TXQ0_SUPT_IPOPT: u32 = 1 << 4;
pub const TXQ0_TPD_BURSTPREF_MASK: u32 = 0xF;
pub const TXQ0_TPD_BURSTPREF_SHIFT: u32 = 0;
pub const TXQ_TPD_BURSTPREF_DEF: u32 = 5;

pub const TXQ1: u32 = 0x1594;
/* bit11:  drop large packet, len > (rfd buf) */
pub const TXQ1_ERRLGPKT_DROP_EN: u32 = 1 << 11;
/* bit[9:0]:: u32 = 8bytes unit */
pub const TXQ1_JUMBO_TSOTHR_MASK: u32 = 0x7FF;
pub const TXQ1_JUMBO_TSOTHR_SHIFT: u32 = 0;
pub const TXQ1_JUMBO_TSO_TH: u32 = (7 * 1024);

/* L1 entrance control */
pub const TXQ2: u32 = 0x1598;
pub const TXQ2_BURST_EN: u32 = 1 << 31;
pub const TXQ2_BURST_HI_WM_MASK: u32 = 0xFFF;
pub const TXQ2_BURST_HI_WM_SHIFT: u32 = 16;
pub const TXQ2_BURST_LO_WM_MASK: u32 = 0xFFF;
pub const TXQ2_BURST_LO_WM_SHIFT: u32 = 0;

pub const RXQ0: u32 = 0x15A0;
pub const RXQ0_EN: u32 = 1 << 31;
pub const RXQ0_CUT_THRU_EN: u32 = 1 << 30;
pub const RXQ0_RSS_HASH_EN: u32 = 1 << 29;
/* bit28:: u32 = 0:goto Q0,: u32 = 1:as table */
pub const RXQ0_NON_IP_QTBL: u32 = 1 << 28;
pub const RXQ0_RSS_MODE_MASK: u32 = 0x3;
pub const RXQ0_RSS_MODE_SHIFT: u32 = 26;
pub const RXQ0_RSS_MODE_DIS: u32 = 0;
pub const RXQ0_RSS_MODE_SQSI: u32 = 1;
pub const RXQ0_RSS_MODE_MQSI: u32 = 2;
pub const RXQ0_RSS_MODE_MQMI: u32 = 3;
pub const RXQ0_NUM_RFD_PREF_MASK: u32 = 0x3F;
pub const RXQ0_NUM_RFD_PREF_SHIFT: u32 = 20;
pub const RXQ0_NUM_RFD_PREF_DEF: u32 = 8;
pub const RXQ0_IDT_TBL_SIZE_MASK: u32 = 0x1FF;
pub const RXQ0_IDT_TBL_SIZE_SHIFT: u32 = 8;
pub const RXQ0_IDT_TBL_SIZE_DEF: u32 = 0x100;
pub const RXQ0_IPV6_PARSE_EN: u32 = 1 << 7;
pub const RXQ0_RSS_HSTYP_MASK: u32 = 0xF;
pub const RXQ0_RSS_HSTYP_SHIFT: u32 = 2;
pub const RXQ0_RSS_HSTYP_IPV6_TCP_EN: u32 = 1 << 5;
pub const RXQ0_RSS_HSTYP_IPV6_EN: u32 = 1 << 4;
pub const RXQ0_RSS_HSTYP_IPV4_TCP_EN: u32 = 1 << 3;
pub const RXQ0_RSS_HSTYP_IPV4_EN: u32 = 1 << 2;
pub const RXQ0_RSS_HSTYP_ALL: u32 = RXQ0_RSS_HSTYP_IPV6_TCP_EN
    | RXQ0_RSS_HSTYP_IPV4_TCP_EN
    | RXQ0_RSS_HSTYP_IPV6_EN
    | RXQ0_RSS_HSTYP_IPV4_EN;
pub const RXQ0_ASPM_THRESH_MASK: u32 = 0x3;
pub const RXQ0_ASPM_THRESH_SHIFT: u32 = 0;
pub const RXQ0_ASPM_THRESH_NO: u32 = 0;
pub const RXQ0_ASPM_THRESH_1M: u32 = 1;
pub const RXQ0_ASPM_THRESH_10M: u32 = 2;
pub const RXQ0_ASPM_THRESH_100M: u32 = 3;

pub const RXQ1: u32 = 0x15A4;
/*: u32 = 32bytes unit */
pub const RXQ1_JUMBO_LKAH_MASK: u32 = 0xF;
pub const RXQ1_JUMBO_LKAH_SHIFT: u32 = 12;
pub const RXQ1_RFD_PREF_DOWN_MASK: u32 = 0x3F;
pub const RXQ1_RFD_PREF_DOWN_SHIFT: u32 = 6;
pub const RXQ1_RFD_PREF_UP_MASK: u32 = 0x3F;
pub const RXQ1_RFD_PREF_UP_SHIFT: u32 = 0;

pub const RXQ2: u32 = 0x15A8;
/* XOFF: USED SRAM LOWER THAN IT, THEN NOTIFY THE PEER TO SEND AGAIN */
pub const RXQ2_RXF_XOFF_THRESH_MASK: u32 = 0xFFF;
pub const RXQ2_RXF_XOFF_THRESH_SHIFT: u32 = 16;
pub const RXQ2_RXF_XON_THRESH_MASK: u32 = 0xFFF;
pub const RXQ2_RXF_XON_THRESH_SHIFT: u32 = 0;
/* Size = tx-packet(1522) + IPG(12) + SOF(8) + 64(Pause) + IPG(12) + SOF(8) +
 *        rx-packet(1522) + delay-of-link(64)
 *      =: u32 = 3212.
 */
pub const RXQ2_RXF_FLOW_CTRL_RSVD: u32 = 3212;

pub const RXQ3: u32 = 0x15AC;
pub const RXQ3_RXD_TIMER_MASK: u32 = 0x7FFF;
pub const RXQ3_RXD_TIMER_SHIFT: u32 = 16;
/*: u32 = 8bytes unit */
pub const RXQ3_RXD_THRESH_MASK: u32 = 0xFFF;
pub const RXQ3_RXD_THRESH_SHIFT: u32 = 0;

pub const DMA: u32 = 0x15C0;
pub const DMA_SMB_NOW: u32 = 1 << 31;
pub const DMA_WPEND_CLR: u32 = 1 << 30;
pub const DMA_RPEND_CLR: u32 = 1 << 29;
pub const DMA_WSRAM_RDCTRL: u32 = 1 << 28;
pub const DMA_RCHNL_SEL_MASK: u32 = 0x3;
pub const DMA_RCHNL_SEL_SHIFT: u32 = 26;
pub const DMA_RCHNL_SEL_1: u32 = 0;
pub const DMA_RCHNL_SEL_2: u32 = 1;
pub const DMA_RCHNL_SEL_3: u32 = 2;
pub const DMA_RCHNL_SEL_4: u32 = 3;
pub const DMA_SMB_EN: u32 = 1 << 21;
pub const DMA_WDLY_CNT_MASK: u32 = 0xF;
pub const DMA_WDLY_CNT_SHIFT: u32 = 16;
pub const DMA_WDLY_CNT_DEF: u32 = 4;
pub const DMA_RDLY_CNT_MASK: u32 = 0x1F;
pub const DMA_RDLY_CNT_SHIFT: u32 = 11;
pub const DMA_RDLY_CNT_DEF: u32 = 15;
/* bit10:: u32 = 0:tpd with pri,: u32 = 1: data */
pub const DMA_RREQ_PRI_DATA: u32 = 1 << 10;
pub const DMA_WREQ_BLEN_MASK: u32 = 0x7;
pub const DMA_WREQ_BLEN_SHIFT: u32 = 7;
pub const DMA_RREQ_BLEN_MASK: u32 = 0x7;
pub const DMA_RREQ_BLEN_SHIFT: u32 = 4;
pub const DMA_PENDING_AUTO_RST: u32 = 1 << 3;
pub const DMA_RORDER_MODE_MASK: u32 = 0x7;
pub const DMA_RORDER_MODE_SHIFT: u32 = 0;
pub const DMA_RORDER_MODE_OUT: u32 = 4;
pub const DMA_RORDER_MODE_ENHANCE: u32 = 2;
pub const DMA_RORDER_MODE_IN: u32 = 1;

pub const WOL0: u32 = 0x14A0;
pub const WOL0_PT7_MATCH: u32 = 1 << 31;
pub const WOL0_PT6_MATCH: u32 = 1 << 30;
pub const WOL0_PT5_MATCH: u32 = 1 << 29;
pub const WOL0_PT4_MATCH: u32 = 1 << 28;
pub const WOL0_PT3_MATCH: u32 = 1 << 27;
pub const WOL0_PT2_MATCH: u32 = 1 << 26;
pub const WOL0_PT1_MATCH: u32 = 1 << 25;
pub const WOL0_PT0_MATCH: u32 = 1 << 24;
pub const WOL0_PT7_EN: u32 = 1 << 23;
pub const WOL0_PT6_EN: u32 = 1 << 22;
pub const WOL0_PT5_EN: u32 = 1 << 21;
pub const WOL0_PT4_EN: u32 = 1 << 20;
pub const WOL0_PT3_EN: u32 = 1 << 19;
pub const WOL0_PT2_EN: u32 = 1 << 18;
pub const WOL0_PT1_EN: u32 = 1 << 17;
pub const WOL0_PT0_EN: u32 = 1 << 16;
pub const WOL0_IPV4_SYNC_EVT: u32 = 1 << 14;
pub const WOL0_IPV6_SYNC_EVT: u32 = 1 << 13;
pub const WOL0_LINK_EVT: u32 = 1 << 10;
pub const WOL0_MAGIC_EVT: u32 = 1 << 9;
pub const WOL0_PATTERN_EVT: u32 = 1 << 8;
pub const WOL0_SWOI_EVT: u32 = 1 << 7;
pub const WOL0_OOB_EN: u32 = 1 << 6;
pub const WOL0_PME_LINK: u32 = 1 << 5;
pub const WOL0_LINK_EN: u32 = 1 << 4;
pub const WOL0_PME_MAGIC_EN: u32 = 1 << 3;
pub const WOL0_MAGIC_EN: u32 = 1 << 2;
pub const WOL0_PME_PATTERN_EN: u32 = 1 << 1;
pub const WOL0_PATTERN_EN: u32 = 1 << 0;

pub const WOL1: u32 = 0x14A4;
pub const WOL1_PT3_LEN_MASK: u32 = 0xFF;
pub const WOL1_PT3_LEN_SHIFT: u32 = 24;
pub const WOL1_PT2_LEN_MASK: u32 = 0xFF;
pub const WOL1_PT2_LEN_SHIFT: u32 = 16;
pub const WOL1_PT1_LEN_MASK: u32 = 0xFF;
pub const WOL1_PT1_LEN_SHIFT: u32 = 8;
pub const WOL1_PT0_LEN_MASK: u32 = 0xFF;
pub const WOL1_PT0_LEN_SHIFT: u32 = 0;

pub const WOL2: u32 = 0x14A8;
pub const WOL2_PT7_LEN_MASK: u32 = 0xFF;
pub const WOL2_PT7_LEN_SHIFT: u32 = 24;
pub const WOL2_PT6_LEN_MASK: u32 = 0xFF;
pub const WOL2_PT6_LEN_SHIFT: u32 = 16;
pub const WOL2_PT5_LEN_MASK: u32 = 0xFF;
pub const WOL2_PT5_LEN_SHIFT: u32 = 8;
pub const WOL2_PT4_LEN_MASK: u32 = 0xFF;
pub const WOL2_PT4_LEN_SHIFT: u32 = 0;

pub const RFD_PIDX: u32 = 0x15E0;
pub const RFD_PIDX_MASK: u32 = 0xFFF;
pub const RFD_PIDX_SHIFT: u32 = 0;

pub const RFD_CIDX: u32 = 0x15F8;
pub const RFD_CIDX_MASK: u32 = 0xFFF;
pub const RFD_CIDX_SHIFT: u32 = 0;

/* MIB */
pub const MIB_BASE: u32 = 0x1700;
pub const MIB_RX_OK: u32 = (MIB_BASE + 0);
pub const MIB_RX_BC: u32 = (MIB_BASE + 4);
pub const MIB_RX_MC: u32 = (MIB_BASE + 8);
pub const MIB_RX_PAUSE: u32 = (MIB_BASE + 12);
pub const MIB_RX_CTRL: u32 = (MIB_BASE + 16);
pub const MIB_RX_FCS: u32 = (MIB_BASE + 20);
pub const MIB_RX_LENERR: u32 = (MIB_BASE + 24);
pub const MIB_RX_BYTCNT: u32 = (MIB_BASE + 28);
pub const MIB_RX_RUNT: u32 = (MIB_BASE + 32);
pub const MIB_RX_FRAGMENT: u32 = (MIB_BASE + 36);
pub const MIB_RX_64B: u32 = (MIB_BASE + 40);
pub const MIB_RX_127B: u32 = (MIB_BASE + 44);
pub const MIB_RX_255B: u32 = (MIB_BASE + 48);
pub const MIB_RX_511B: u32 = (MIB_BASE + 52);
pub const MIB_RX_1023B: u32 = (MIB_BASE + 56);
pub const MIB_RX_1518B: u32 = (MIB_BASE + 60);
pub const MIB_RX_SZMAX: u32 = (MIB_BASE + 64);
pub const MIB_RX_OVSZ: u32 = (MIB_BASE + 68);
pub const MIB_RXF_OV: u32 = (MIB_BASE + 72);
pub const MIB_RRD_OV: u32 = (MIB_BASE + 76);
pub const MIB_RX_ALIGN: u32 = (MIB_BASE + 80);
pub const MIB_RX_BCCNT: u32 = (MIB_BASE + 84);
pub const MIB_RX_MCCNT: u32 = (MIB_BASE + 88);
pub const MIB_RX_ERRADDR: u32 = (MIB_BASE + 92);
pub const MIB_TX_OK: u32 = (MIB_BASE + 96);
pub const MIB_TX_BC: u32 = (MIB_BASE + 100);
pub const MIB_TX_MC: u32 = (MIB_BASE + 104);
pub const MIB_TX_PAUSE: u32 = (MIB_BASE + 108);
pub const MIB_TX_EXCDEFER: u32 = (MIB_BASE + 112);
pub const MIB_TX_CTRL: u32 = (MIB_BASE + 116);
pub const MIB_TX_DEFER: u32 = (MIB_BASE + 120);
pub const MIB_TX_BYTCNT: u32 = (MIB_BASE + 124);
pub const MIB_TX_64B: u32 = (MIB_BASE + 128);
pub const MIB_TX_127B: u32 = (MIB_BASE + 132);
pub const MIB_TX_255B: u32 = (MIB_BASE + 136);
pub const MIB_TX_511B: u32 = (MIB_BASE + 140);
pub const MIB_TX_1023B: u32 = (MIB_BASE + 144);
pub const MIB_TX_1518B: u32 = (MIB_BASE + 148);
pub const MIB_TX_SZMAX: u32 = (MIB_BASE + 152);
pub const MIB_TX_1COL: u32 = (MIB_BASE + 156);
pub const MIB_TX_2COL: u32 = (MIB_BASE + 160);
pub const MIB_TX_LATCOL: u32 = (MIB_BASE + 164);
pub const MIB_TX_ABRTCOL: u32 = (MIB_BASE + 168);
pub const MIB_TX_UNDRUN: u32 = (MIB_BASE + 172);
pub const MIB_TX_TRDBEOP: u32 = (MIB_BASE + 176);
pub const MIB_TX_LENERR: u32 = (MIB_BASE + 180);
pub const MIB_TX_TRUNC: u32 = (MIB_BASE + 184);
pub const MIB_TX_BCCNT: u32 = (MIB_BASE + 188);
pub const MIB_TX_MCCNT: u32 = (MIB_BASE + 192);
pub const MIB_UPDATE: u32 = (MIB_BASE + 196);

pub const ISR: u32 = 0x1600;
pub const ISR_DIS: u32 = 1 << 31;
pub const ISR_RX_Q7: u32 = 1 << 30;
pub const ISR_RX_Q6: u32 = 1 << 29;
pub const ISR_RX_Q5: u32 = 1 << 28;
pub const ISR_RX_Q4: u32 = 1 << 27;
pub const ISR_PCIE_LNKDOWN: u32 = 1 << 26;
pub const ISR_PCIE_CERR: u32 = 1 << 25;
pub const ISR_PCIE_NFERR: u32 = 1 << 24;
pub const ISR_PCIE_FERR: u32 = 1 << 23;
pub const ISR_PCIE_UR: u32 = 1 << 22;
pub const ISR_MAC_TX: u32 = 1 << 21;
pub const ISR_MAC_RX: u32 = 1 << 20;
pub const ISR_RX_Q3: u32 = 1 << 19;
pub const ISR_RX_Q2: u32 = 1 << 18;
pub const ISR_RX_Q1: u32 = 1 << 17;
pub const ISR_RX_Q0: u32 = 1 << 16;
pub const ISR_TX_Q0: u32 = 1 << 15;
pub const ISR_TXQ_TO: u32 = 1 << 14;
pub const ISR_PHY_LPW: u32 = 1 << 13;
pub const ISR_PHY: u32 = 1 << 12;
pub const ISR_TX_CREDIT: u32 = 1 << 11;
pub const ISR_DMAW: u32 = 1 << 10;
pub const ISR_DMAR: u32 = 1 << 9;
pub const ISR_TXF_UR: u32 = 1 << 8;
pub const ISR_TX_Q3: u32 = 1 << 7;
pub const ISR_TX_Q2: u32 = 1 << 6;
pub const ISR_TX_Q1: u32 = 1 << 5;
pub const ISR_RFD_UR: u32 = 1 << 4;
pub const ISR_RXF_OV: u32 = 1 << 3;
pub const ISR_MANU: u32 = 1 << 2;
pub const ISR_TIMER: u32 = 1 << 1;
pub const ISR_SMB: u32 = 1 << 0;

pub const IMR: u32 = 0x1604;

/* re-send assert msg if SW no response */
pub const INT_RETRIG: u32 = 0x1608;
pub const INT_RETRIG_TIMER_MASK: u32 = 0xFFFF;
pub const INT_RETRIG_TIMER_SHIFT: u32 = 0;
/*: u32 = 40ms */
pub const INT_RETRIG_TO: u32 = 20000;

/* re-send deassert msg if SW no response */
pub const INT_DEASST_TIMER: u32 = 0x1614;

/* reg1620 used for sleep status */
pub const PATTERN_MASK: u32 = 0x1620;
pub const PATTERN_MASK_LEN: u32 = 128;

pub const FLT1_SRC_IP0: u32 = 0x1A00;
pub const FLT1_SRC_IP1: u32 = 0x1A04;
pub const FLT1_SRC_IP2: u32 = 0x1A08;
pub const FLT1_SRC_IP3: u32 = 0x1A0C;
pub const FLT1_DST_IP0: u32 = 0x1A10;
pub const FLT1_DST_IP1: u32 = 0x1A14;
pub const FLT1_DST_IP2: u32 = 0x1A18;
pub const FLT1_DST_IP3: u32 = 0x1A1C;
pub const FLT1_PORT: u32 = 0x1A20;
pub const FLT1_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT1_PORT_DST_SHIFT: u32 = 16;
pub const FLT1_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT1_PORT_SRC_SHIFT: u32 = 0;

pub const FLT2_SRC_IP0: u32 = 0x1A24;
pub const FLT2_SRC_IP1: u32 = 0x1A28;
pub const FLT2_SRC_IP2: u32 = 0x1A2C;
pub const FLT2_SRC_IP3: u32 = 0x1A30;
pub const FLT2_DST_IP0: u32 = 0x1A34;
pub const FLT2_DST_IP1: u32 = 0x1A38;
pub const FLT2_DST_IP2: u32 = 0x1A40;
pub const FLT2_DST_IP3: u32 = 0x1A44;
pub const FLT2_PORT: u32 = 0x1A48;
pub const FLT2_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT2_PORT_DST_SHIFT: u32 = 16;
pub const FLT2_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT2_PORT_SRC_SHIFT: u32 = 0;

pub const FLT3_SRC_IP0: u32 = 0x1A4C;
pub const FLT3_SRC_IP1: u32 = 0x1A50;
pub const FLT3_SRC_IP2: u32 = 0x1A54;
pub const FLT3_SRC_IP3: u32 = 0x1A58;
pub const FLT3_DST_IP0: u32 = 0x1A5C;
pub const FLT3_DST_IP1: u32 = 0x1A60;
pub const FLT3_DST_IP2: u32 = 0x1A64;
pub const FLT3_DST_IP3: u32 = 0x1A68;
pub const FLT3_PORT: u32 = 0x1A6C;
pub const FLT3_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT3_PORT_DST_SHIFT: u32 = 16;
pub const FLT3_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT3_PORT_SRC_SHIFT: u32 = 0;

pub const FLT4_SRC_IP0: u32 = 0x1A70;
pub const FLT4_SRC_IP1: u32 = 0x1A74;
pub const FLT4_SRC_IP2: u32 = 0x1A78;
pub const FLT4_SRC_IP3: u32 = 0x1A7C;
pub const FLT4_DST_IP0: u32 = 0x1A80;
pub const FLT4_DST_IP1: u32 = 0x1A84;
pub const FLT4_DST_IP2: u32 = 0x1A88;
pub const FLT4_DST_IP3: u32 = 0x1A8C;
pub const FLT4_PORT: u32 = 0x1A90;
pub const FLT4_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT4_PORT_DST_SHIFT: u32 = 16;
pub const FLT4_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT4_PORT_SRC_SHIFT: u32 = 0;

pub const FLT5_SRC_IP0: u32 = 0x1A94;
pub const FLT5_SRC_IP1: u32 = 0x1A98;
pub const FLT5_SRC_IP2: u32 = 0x1A9C;
pub const FLT5_SRC_IP3: u32 = 0x1AA0;
pub const FLT5_DST_IP0: u32 = 0x1AA4;
pub const FLT5_DST_IP1: u32 = 0x1AA8;
pub const FLT5_DST_IP2: u32 = 0x1AAC;
pub const FLT5_DST_IP3: u32 = 0x1AB0;
pub const FLT5_PORT: u32 = 0x1AB4;
pub const FLT5_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT5_PORT_DST_SHIFT: u32 = 16;
pub const FLT5_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT5_PORT_SRC_SHIFT: u32 = 0;

pub const FLT6_SRC_IP0: u32 = 0x1AB8;
pub const FLT6_SRC_IP1: u32 = 0x1ABC;
pub const FLT6_SRC_IP2: u32 = 0x1AC0;
pub const FLT6_SRC_IP3: u32 = 0x1AC8;
pub const FLT6_DST_IP0: u32 = 0x1620;
pub const FLT6_DST_IP1: u32 = 0x1624;
pub const FLT6_DST_IP2: u32 = 0x1628;
pub const FLT6_DST_IP3: u32 = 0x162C;
pub const FLT6_PORT: u32 = 0x1630;
pub const FLT6_PORT_DST_MASK: u32 = 0xFFFF;
pub const FLT6_PORT_DST_SHIFT: u32 = 16;
pub const FLT6_PORT_SRC_MASK: u32 = 0xFFFF;
pub const FLT6_PORT_SRC_SHIFT: u32 = 0;

pub const FLTCTRL: u32 = 0x1634;
pub const FLTCTRL_PSTHR_TIMER_MASK: u32 = 0xFF;
pub const FLTCTRL_PSTHR_TIMER_SHIFT: u32 = 24;
pub const FLTCTRL_CHK_DSTPRT6: u32 = 1 << 23;
pub const FLTCTRL_CHK_SRCPRT6: u32 = 1 << 22;
pub const FLTCTRL_CHK_DSTIP6: u32 = 1 << 21;
pub const FLTCTRL_CHK_SRCIP6: u32 = 1 << 20;
pub const FLTCTRL_CHK_DSTPRT5: u32 = 1 << 19;
pub const FLTCTRL_CHK_SRCPRT5: u32 = 1 << 18;
pub const FLTCTRL_CHK_DSTIP5: u32 = 1 << 17;
pub const FLTCTRL_CHK_SRCIP5: u32 = 1 << 16;
pub const FLTCTRL_CHK_DSTPRT4: u32 = 1 << 15;
pub const FLTCTRL_CHK_SRCPRT4: u32 = 1 << 14;
pub const FLTCTRL_CHK_DSTIP4: u32 = 1 << 13;
pub const FLTCTRL_CHK_SRCIP4: u32 = 1 << 12;
pub const FLTCTRL_CHK_DSTPRT3: u32 = 1 << 11;
pub const FLTCTRL_CHK_SRCPRT3: u32 = 1 << 10;
pub const FLTCTRL_CHK_DSTIP3: u32 = 1 << 9;
pub const FLTCTRL_CHK_SRCIP3: u32 = 1 << 8;
pub const FLTCTRL_CHK_DSTPRT2: u32 = 1 << 7;
pub const FLTCTRL_CHK_SRCPRT2: u32 = 1 << 6;
pub const FLTCTRL_CHK_DSTIP2: u32 = 1 << 5;
pub const FLTCTRL_CHK_SRCIP2: u32 = 1 << 4;
pub const FLTCTRL_CHK_DSTPRT1: u32 = 1 << 3;
pub const FLTCTRL_CHK_SRCPRT1: u32 = 1 << 2;
pub const FLTCTRL_CHK_DSTIP1: u32 = 1 << 1;
pub const FLTCTRL_CHK_SRCIP1: u32 = 1 << 0;

pub const DROP_ALG1: u32 = 0x1638;
pub const DROP_ALG1_BWCHGVAL_MASK: u32 = 0xFFFFF;
pub const DROP_ALG1_BWCHGVAL_SHIFT: u32 = 12;
/* bit11:: u32 = 0:3.125%,: u32 = 1:6.25% */
pub const DROP_ALG1_BWCHGSCL_6: u32 = 1 << 11;
pub const DROP_ALG1_ASUR_LWQ_EN: u32 = 1 << 10;
pub const DROP_ALG1_BWCHGVAL_EN: u32 = 1 << 9;
pub const DROP_ALG1_BWCHGSCL_EN: u32 = 1 << 8;
pub const DROP_ALG1_PSTHR_AUTO: u32 = 1 << 7;
pub const DROP_ALG1_MIN_PSTHR_MASK: u32 = 0x3;
pub const DROP_ALG1_MIN_PSTHR_SHIFT: u32 = 5;
pub const DROP_ALG1_MIN_PSTHR_1_16: u32 = 0;
pub const DROP_ALG1_MIN_PSTHR_1_8: u32 = 1;
pub const DROP_ALG1_MIN_PSTHR_1_4: u32 = 2;
pub const DROP_ALG1_MIN_PSTHR_1_2: u32 = 3;
pub const DROP_ALG1_PSCL_MASK: u32 = 0x3;
pub const DROP_ALG1_PSCL_SHIFT: u32 = 3;
pub const DROP_ALG1_PSCL_1_4: u32 = 0;
pub const DROP_ALG1_PSCL_1_8: u32 = 1;
pub const DROP_ALG1_PSCL_1_16: u32 = 2;
pub const DROP_ALG1_PSCL_1_32: u32 = 3;
pub const DROP_ALG1_TIMESLOT_MASK: u32 = 0x7;
pub const DROP_ALG1_TIMESLOT_SHIFT: u32 = 0;
pub const DROP_ALG1_TIMESLOT_4MS: u32 = 0;
pub const DROP_ALG1_TIMESLOT_8MS: u32 = 1;
pub const DROP_ALG1_TIMESLOT_16MS: u32 = 2;
pub const DROP_ALG1_TIMESLOT_32MS: u32 = 3;
pub const DROP_ALG1_TIMESLOT_64MS: u32 = 4;
pub const DROP_ALG1_TIMESLOT_128MS: u32 = 5;
pub const DROP_ALG1_TIMESLOT_256MS: u32 = 6;
pub const DROP_ALG1_TIMESLOT_512MS: u32 = 7;

pub const DROP_ALG2: u32 = 0x163C;
pub const DROP_ALG2_SMPLTIME_MASK: u32 = 0xF;
pub const DROP_ALG2_SMPLTIME_SHIFT: u32 = 24;
pub const DROP_ALG2_LWQBW_MASK: u32 = 0xFFFFFF;
pub const DROP_ALG2_LWQBW_SHIFT: u32 = 0;

pub const SMB_TIMER: u32 = 0x15C4;

pub const TINT_TPD_THRSHLD: u32 = 0x15C8;

pub const TINT_TIMER: u32 = 0x15CC;

pub const CLK_GATE: u32 = 0x1814;
/* bit[8:6]: for: u32 = B0+ */
pub const CLK_GATE_125M_SW_DIS_CR: u32 = 1 << 8;
pub const CLK_GATE_125M_SW_AZ: u32 = 1 << 7;
pub const CLK_GATE_125M_SW_IDLE: u32 = 1 << 6;
pub const CLK_GATE_RXMAC: u32 = 1 << 5;
pub const CLK_GATE_TXMAC: u32 = 1 << 4;
pub const CLK_GATE_RXQ: u32 = 1 << 3;
pub const CLK_GATE_TXQ: u32 = 1 << 2;
pub const CLK_GATE_DMAR: u32 = 1 << 1;
pub const CLK_GATE_DMAW: u32 = 1 << 0;
pub const CLK_GATE_ALL_A0: u32 =
    CLK_GATE_RXMAC | CLK_GATE_TXMAC | CLK_GATE_RXQ | CLK_GATE_TXQ | CLK_GATE_DMAR | CLK_GATE_DMAW;
pub const CLK_GATE_ALL_B0: u32 = CLK_GATE_ALL_A0;

/* PORST affect */
pub const BTROM_CFG: u32 = 0x1800;

/* interop between drivers */
pub const DRV: u32 = 0x1804;
pub const DRV_PHY_AUTO: u32 = 1 << 28;
pub const DRV_PHY_1000: u32 = 1 << 27;
pub const DRV_PHY_100: u32 = 1 << 26;
pub const DRV_PHY_10: u32 = 1 << 25;
pub const DRV_PHY_DUPLEX: u32 = 1 << 24;
/* bit23: adv Pause */
pub const DRV_PHY_PAUSE: u32 = 1 << 23;
/* bit22: adv Asym Pause */
pub const DRV_PHY_APAUSE: u32 = 1 << 22;
/* bit21:: u32 = 1:en AZ */
pub const DRV_PHY_EEE: u32 = 1 << 21;
pub const DRV_PHY_MASK: u32 = 0xFF;
pub const DRV_PHY_SHIFT: u32 = 21;
pub const DRV_PHY_UNKNOWN: u32 = 0;
pub const DRV_DISABLE: u32 = 1 << 18;
pub const DRV_WOLS5_EN: u32 = 1 << 17;
pub const DRV_WOLS5_BIOS_EN: u32 = 1 << 16;
pub const DRV_AZ_EN: u32 = 1 << 12;
pub const DRV_WOLPATTERN_EN: u32 = 1 << 11;
pub const DRV_WOLLINKUP_EN: u32 = 1 << 10;
pub const DRV_WOLMAGIC_EN: u32 = 1 << 9;
pub const DRV_WOLCAP_BIOS_EN: u32 = 1 << 8;
pub const DRV_ASPM_SPD1000LMT_MASK: u32 = 0x3;
pub const DRV_ASPM_SPD1000LMT_SHIFT: u32 = 4;
pub const DRV_ASPM_SPD1000LMT_100M: u32 = 0;
pub const DRV_ASPM_SPD1000LMT_NO: u32 = 1;
pub const DRV_ASPM_SPD1000LMT_1M: u32 = 2;
pub const DRV_ASPM_SPD1000LMT_10M: u32 = 3;
pub const DRV_ASPM_SPD100LMT_MASK: u32 = 0x3;
pub const DRV_ASPM_SPD100LMT_SHIFT: u32 = 2;
pub const DRV_ASPM_SPD100LMT_1M: u32 = 0;
pub const DRV_ASPM_SPD100LMT_10M: u32 = 1;
pub const DRV_ASPM_SPD100LMT_100M: u32 = 2;
pub const DRV_ASPM_SPD100LMT_NO: u32 = 3;
pub const DRV_ASPM_SPD10LMT_MASK: u32 = 0x3;
pub const DRV_ASPM_SPD10LMT_SHIFT: u32 = 0;
pub const DRV_ASPM_SPD10LMT_1M: u32 = 0;
pub const DRV_ASPM_SPD10LMT_10M: u32 = 1;
pub const DRV_ASPM_SPD10LMT_100M: u32 = 2;
pub const DRV_ASPM_SPD10LMT_NO: u32 = 3;

/* flag of phy inited */
pub const PHY_INITED: u16 = 0x003F;

/* PERST affect */
pub const DRV_ERR1: u32 = 0x1808;
pub const DRV_ERR1_GEN: u32 = 1 << 31;
pub const DRV_ERR1_NOR: u32 = 1 << 30;
pub const DRV_ERR1_TRUNC: u32 = 1 << 29;
pub const DRV_ERR1_RES: u32 = 1 << 28;
pub const DRV_ERR1_INTFATAL: u32 = 1 << 27;
pub const DRV_ERR1_TXQPEND: u32 = 1 << 26;
pub const DRV_ERR1_DMAW: u32 = 1 << 25;
pub const DRV_ERR1_DMAR: u32 = 1 << 24;
pub const DRV_ERR1_PCIELNKDWN: u32 = 1 << 23;
pub const DRV_ERR1_PKTSIZE: u32 = 1 << 22;
pub const DRV_ERR1_FIFOFUL: u32 = 1 << 21;
pub const DRV_ERR1_RFDUR: u32 = 1 << 20;
pub const DRV_ERR1_RRDSI: u32 = 1 << 19;
pub const DRV_ERR1_UPDATE: u32 = 1 << 18;

pub const DRV_ERR2: u32 = 0x180C;

pub const DBG_ADDR: u32 = 0x1900;
pub const DBG_DATA: u32 = 0x1904;

pub const SYNC_IPV4_SA: u32 = 0x1A00;
pub const SYNC_IPV4_DA: u32 = 0x1A04;

pub const SYNC_V4PORT: u32 = 0x1A08;
pub const SYNC_V4PORT_DST_MASK: u32 = 0xFFFF;
pub const SYNC_V4PORT_DST_SHIFT: u32 = 16;
pub const SYNC_V4PORT_SRC_MASK: u32 = 0xFFFF;
pub const SYNC_V4PORT_SRC_SHIFT: u32 = 0;

pub const SYNC_IPV6_SA0: u32 = 0x1A0C;
pub const SYNC_IPV6_SA1: u32 = 0x1A10;
pub const SYNC_IPV6_SA2: u32 = 0x1A14;
pub const SYNC_IPV6_SA3: u32 = 0x1A18;
pub const SYNC_IPV6_DA0: u32 = 0x1A1C;
pub const SYNC_IPV6_DA1: u32 = 0x1A20;
pub const SYNC_IPV6_DA2: u32 = 0x1A24;
pub const SYNC_IPV6_DA3: u32 = 0x1A28;

pub const SYNC_V6PORT: u32 = 0x1A2C;
pub const SYNC_V6PORT_DST_MASK: u32 = 0xFFFF;
pub const SYNC_V6PORT_DST_SHIFT: u32 = 16;
pub const SYNC_V6PORT_SRC_MASK: u32 = 0xFFFF;
pub const SYNC_V6PORT_SRC_SHIFT: u32 = 0;

pub const ARP_REMOTE_IPV4: u32 = 0x1A30;
pub const ARP_HOST_IPV4: u32 = 0x1A34;
pub const ARP_MAC0: u32 = 0x1A38;
pub const ARP_MAC1: u32 = 0x1A3C;

pub const FIRST_REMOTE_IPV6_0: u32 = 0x1A40;
pub const FIRST_REMOTE_IPV6_1: u32 = 0x1A44;
pub const FIRST_REMOTE_IPV6_2: u32 = 0x1A48;
pub const FIRST_REMOTE_IPV6_3: u32 = 0x1A4C;

pub const FIRST_SN_IPV6_0: u32 = 0x1A50;
pub const FIRST_SN_IPV6_1: u32 = 0x1A54;
pub const FIRST_SN_IPV6_2: u32 = 0x1A58;
pub const FIRST_SN_IPV6_3: u32 = 0x1A5C;

pub const FIRST_TAR_IPV6_1_0: u32 = 0x1A60;
pub const FIRST_TAR_IPV6_1_1: u32 = 0x1A64;
pub const FIRST_TAR_IPV6_1_2: u32 = 0x1A68;
pub const FIRST_TAR_IPV6_1_3: u32 = 0x1A6C;
pub const FIRST_TAR_IPV6_2_0: u32 = 0x1A70;
pub const FIRST_TAR_IPV6_2_1: u32 = 0x1A74;
pub const FIRST_TAR_IPV6_2_2: u32 = 0x1A78;
pub const FIRST_TAR_IPV6_2_3: u32 = 0x1A7C;

pub const SECOND_REMOTE_IPV6_0: u32 = 0x1A80;
pub const SECOND_REMOTE_IPV6_1: u32 = 0x1A84;
pub const SECOND_REMOTE_IPV6_2: u32 = 0x1A88;
pub const SECOND_REMOTE_IPV6_3: u32 = 0x1A8C;

pub const SECOND_SN_IPV6_0: u32 = 0x1A90;
pub const SECOND_SN_IPV6_1: u32 = 0x1A94;
pub const SECOND_SN_IPV6_2: u32 = 0x1A98;
pub const SECOND_SN_IPV6_3: u32 = 0x1A9C;

pub const SECOND_TAR_IPV6_1_0: u32 = 0x1AA0;
pub const SECOND_TAR_IPV6_1_1: u32 = 0x1AA4;
pub const SECOND_TAR_IPV6_1_2: u32 = 0x1AA8;
pub const SECOND_TAR_IPV6_1_3: u32 = 0x1AAC;
pub const SECOND_TAR_IPV6_2_0: u32 = 0x1AB0;
pub const SECOND_TAR_IPV6_2_1: u32 = 0x1AB4;
pub const SECOND_TAR_IPV6_2_2: u32 = 0x1AB8;
pub const SECOND_TAR_IPV6_2_3: u32 = 0x1ABC;

pub const FIRST_NS_MAC0: u32 = 0x1AC0;
pub const FIRST_NS_MAC1: u32 = 0x1AC4;

pub const SECOND_NS_MAC0: u32 = 0x1AC8;
pub const SECOND_NS_MAC1: u32 = 0x1ACC;

pub const PMOFLD: u32 = 0x144C;
/* bit[11:10]: for: u32 = B0+ */
pub const PMOFLD_ECMA_IGNR_FRG_SSSR: u32 = 1 << 11;
pub const PMOFLD_ARP_CNFLCT_WAKEUP: u32 = 1 << 10;
pub const PMOFLD_MULTI_SOLD: u32 = 1 << 9;
pub const PMOFLD_ICMP_XSUM: u32 = 1 << 8;
pub const PMOFLD_GARP_REPLY: u32 = 1 << 7;
pub const PMOFLD_SYNCV6_ANY: u32 = 1 << 6;
pub const PMOFLD_SYNCV4_ANY: u32 = 1 << 5;
pub const PMOFLD_BY_HW: u32 = 1 << 4;
pub const PMOFLD_NS_EN: u32 = 1 << 3;
pub const PMOFLD_ARP_EN: u32 = 1 << 2;
pub const PMOFLD_SYNCV6_EN: u32 = 1 << 1;
pub const PMOFLD_SYNCV4_EN: u32 = 1 << 0;

/* reg: u32 = 1830 ~: u32 = 186C for C0+,: u32 = 16 bit map patterns and wake packet detection */
pub const WOL_CTRL2: u32 = 0x1830;
pub const WOL_CTRL2_DATA_STORE: u32 = 1 << 3;
pub const WOL_CTRL2_PTRN_EVT: u32 = 1 << 2;
pub const WOL_CTRL2_PME_PTRN_EN: u32 = 1 << 1;
pub const WOL_CTRL2_PTRN_EN: u32 = 1 << 0;

pub const WOL_CTRL3: u32 = 0x1834;
pub const WOL_CTRL3_PTRN_ADDR_MASK: u32 = 0xFFFFF;
pub const WOL_CTRL3_PTRN_ADDR_SHIFT: u32 = 0;

pub const WOL_CTRL4: u32 = 0x1838;
pub const WOL_CTRL4_PT15_MATCH: u32 = 1 << 31;
pub const WOL_CTRL4_PT14_MATCH: u32 = 1 << 30;
pub const WOL_CTRL4_PT13_MATCH: u32 = 1 << 29;
pub const WOL_CTRL4_PT12_MATCH: u32 = 1 << 28;
pub const WOL_CTRL4_PT11_MATCH: u32 = 1 << 27;
pub const WOL_CTRL4_PT10_MATCH: u32 = 1 << 26;
pub const WOL_CTRL4_PT9_MATCH: u32 = 1 << 25;
pub const WOL_CTRL4_PT8_MATCH: u32 = 1 << 24;
pub const WOL_CTRL4_PT7_MATCH: u32 = 1 << 23;
pub const WOL_CTRL4_PT6_MATCH: u32 = 1 << 22;
pub const WOL_CTRL4_PT5_MATCH: u32 = 1 << 21;
pub const WOL_CTRL4_PT4_MATCH: u32 = 1 << 20;
pub const WOL_CTRL4_PT3_MATCH: u32 = 1 << 19;
pub const WOL_CTRL4_PT2_MATCH: u32 = 1 << 18;
pub const WOL_CTRL4_PT1_MATCH: u32 = 1 << 17;
pub const WOL_CTRL4_PT0_MATCH: u32 = 1 << 16;
pub const WOL_CTRL4_PT15_EN: u32 = 1 << 15;
pub const WOL_CTRL4_PT14_EN: u32 = 1 << 14;
pub const WOL_CTRL4_PT13_EN: u32 = 1 << 13;
pub const WOL_CTRL4_PT12_EN: u32 = 1 << 12;
pub const WOL_CTRL4_PT11_EN: u32 = 1 << 11;
pub const WOL_CTRL4_PT10_EN: u32 = 1 << 10;
pub const WOL_CTRL4_PT9_EN: u32 = 1 << 9;
pub const WOL_CTRL4_PT8_EN: u32 = 1 << 8;
pub const WOL_CTRL4_PT7_EN: u32 = 1 << 7;
pub const WOL_CTRL4_PT6_EN: u32 = 1 << 6;
pub const WOL_CTRL4_PT5_EN: u32 = 1 << 5;
pub const WOL_CTRL4_PT4_EN: u32 = 1 << 4;
pub const WOL_CTRL4_PT3_EN: u32 = 1 << 3;
pub const WOL_CTRL4_PT2_EN: u32 = 1 << 2;
pub const WOL_CTRL4_PT1_EN: u32 = 1 << 1;
pub const WOL_CTRL4_PT0_EN: u32 = 1 << 0;

pub const WOL_CTRL5: u32 = 0x183C;
pub const WOL_CTRL5_PT3_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT3_LEN_SHIFT: u32 = 24;
pub const WOL_CTRL5_PT2_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT2_LEN_SHIFT: u32 = 16;
pub const WOL_CTRL5_PT1_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT1_LEN_SHIFT: u32 = 8;
pub const WOL_CTRL5_PT0_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT0_LEN_SHIFT: u32 = 0;

pub const WOL_CTRL6: u32 = 0x1840;
pub const WOL_CTRL5_PT7_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT7_LEN_SHIFT: u32 = 24;
pub const WOL_CTRL5_PT6_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT6_LEN_SHIFT: u32 = 16;
pub const WOL_CTRL5_PT5_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT5_LEN_SHIFT: u32 = 8;
pub const WOL_CTRL5_PT4_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT4_LEN_SHIFT: u32 = 0;

pub const WOL_CTRL7: u32 = 0x1844;
pub const WOL_CTRL5_PT11_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT11_LEN_SHIFT: u32 = 24;
pub const WOL_CTRL5_PT10_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT10_LEN_SHIFT: u32 = 16;
pub const WOL_CTRL5_PT9_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT9_LEN_SHIFT: u32 = 8;
pub const WOL_CTRL5_PT8_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT8_LEN_SHIFT: u32 = 0;

pub const WOL_CTRL8: u32 = 0x1848;
pub const WOL_CTRL5_PT15_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT15_LEN_SHIFT: u32 = 24;
pub const WOL_CTRL5_PT14_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT14_LEN_SHIFT: u32 = 16;
pub const WOL_CTRL5_PT13_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT13_LEN_SHIFT: u32 = 8;
pub const WOL_CTRL5_PT12_LEN_MASK: u32 = 0xFF;
pub const WOL_CTRL5_PT12_LEN_SHIFT: u32 = 0;

pub const ACER_FIXED_PTN0: u32 = 0x1850;
pub const ACER_FIXED_PTN0_MASK: u32 = 0xFFFFFFFF;
pub const ACER_FIXED_PTN0_SHIFT: u32 = 0;

pub const ACER_FIXED_PTN1: u32 = 0x1854;
pub const ACER_FIXED_PTN1_MASK: u32 = 0xFFFF;
pub const ACER_FIXED_PTN1_SHIFT: u32 = 0;

pub const ACER_RANDOM_NUM0: u32 = 0x1858;
pub const ACER_RANDOM_NUM0_MASK: u32 = 0xFFFFFFFF;
pub const ACER_RANDOM_NUM0_SHIFT: u32 = 0;

pub const ACER_RANDOM_NUM1: u32 = 0x185C;
pub const ACER_RANDOM_NUM1_MASK: u32 = 0xFFFFFFFF;
pub const ACER_RANDOM_NUM1_SHIFT: u32 = 0;

pub const ACER_RANDOM_NUM2: u32 = 0x1860;
pub const ACER_RANDOM_NUM2_MASK: u32 = 0xFFFFFFFF;
pub const ACER_RANDOM_NUM2_SHIFT: u32 = 0;

pub const ACER_RANDOM_NUM3: u32 = 0x1864;
pub const ACER_RANDOM_NUM3_MASK: u32 = 0xFFFFFFFF;
pub const ACER_RANDOM_NUM3_SHIFT: u32 = 0;

pub const ACER_MAGIC: u32 = 0x1868;
pub const ACER_MAGIC_EN: u32 = 1 << 31;
pub const ACER_MAGIC_PME_EN: u32 = 1 << 30;
pub const ACER_MAGIC_MATCH: u32 = 1 << 29;
pub const ACER_MAGIC_FF_CHECK: u32 = 1 << 10;
pub const ACER_MAGIC_RAN_LEN_MASK: u32 = 0x1F;
pub const ACER_MAGIC_RAN_LEN_SHIFT: u32 = 5;
pub const ACER_MAGIC_FIX_LEN_MASK: u32 = 0x1F;
pub const ACER_MAGIC_FIX_LEN_SHIFT: u32 = 0;

pub const ACER_TIMER: u32 = 0x186C;
pub const ACER_TIMER_EN: u32 = 1 << 31;
pub const ACER_TIMER_PME_EN: u32 = 1 << 30;
pub const ACER_TIMER_MATCH: u32 = 1 << 29;
pub const ACER_TIMER_THRES_MASK: u32 = 0x1FFFF;
pub const ACER_TIMER_THRES_SHIFT: u32 = 0;
pub const ACER_TIMER_THRES_DEF: u32 = 1;

/* RSS definitions */
pub const RSS_KEY0: u32 = 0x14B0;
pub const RSS_KEY1: u32 = 0x14B4;
pub const RSS_KEY2: u32 = 0x14B8;
pub const RSS_KEY3: u32 = 0x14BC;
pub const RSS_KEY4: u32 = 0x14C0;
pub const RSS_KEY5: u32 = 0x14C4;
pub const RSS_KEY6: u32 = 0x14C8;
pub const RSS_KEY7: u32 = 0x14CC;
pub const RSS_KEY8: u32 = 0x14D0;
pub const RSS_KEY9: u32 = 0x14D4;

pub const RSS_IDT_TBL0: u32 = 0x1B00;
pub const RSS_IDT_TBL1: u32 = 0x1B04;
pub const RSS_IDT_TBL2: u32 = 0x1B08;
pub const RSS_IDT_TBL3: u32 = 0x1B0C;
pub const RSS_IDT_TBL4: u32 = 0x1B10;
pub const RSS_IDT_TBL5: u32 = 0x1B14;
pub const RSS_IDT_TBL6: u32 = 0x1B18;
pub const RSS_IDT_TBL7: u32 = 0x1B1C;
pub const RSS_IDT_TBL8: u32 = 0x1B20;
pub const RSS_IDT_TBL9: u32 = 0x1B24;
pub const RSS_IDT_TBL10: u32 = 0x1B28;
pub const RSS_IDT_TBL11: u32 = 0x1B2C;
pub const RSS_IDT_TBL12: u32 = 0x1B30;
pub const RSS_IDT_TBL13: u32 = 0x1B34;
pub const RSS_IDT_TBL14: u32 = 0x1B38;
pub const RSS_IDT_TBL15: u32 = 0x1B3C;
pub const RSS_IDT_TBL16: u32 = 0x1B40;
pub const RSS_IDT_TBL17: u32 = 0x1B44;
pub const RSS_IDT_TBL18: u32 = 0x1B48;
pub const RSS_IDT_TBL19: u32 = 0x1B4C;
pub const RSS_IDT_TBL20: u32 = 0x1B50;
pub const RSS_IDT_TBL21: u32 = 0x1B54;
pub const RSS_IDT_TBL22: u32 = 0x1B58;
pub const RSS_IDT_TBL23: u32 = 0x1B5C;
pub const RSS_IDT_TBL24: u32 = 0x1B60;
pub const RSS_IDT_TBL25: u32 = 0x1B64;
pub const RSS_IDT_TBL26: u32 = 0x1B68;
pub const RSS_IDT_TBL27: u32 = 0x1B6C;
pub const RSS_IDT_TBL28: u32 = 0x1B70;
pub const RSS_IDT_TBL29: u32 = 0x1B74;
pub const RSS_IDT_TBL30: u32 = 0x1B78;
pub const RSS_IDT_TBL31: u32 = 0x1B7C;

pub const RSS_HASH_VAL: u32 = 0x15B0;
pub const RSS_HASH_FLAG: u32 = 0x15B4;

pub const RSS_BASE_CPU_NUM: u32 = 0x15B8;

pub const MSI_MAP_TBL1: u32 = 0x15D0;
pub const MSI_MAP_TBL1_ALERT_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_ALERT_SHIFT: u32 = 28;
pub const MSI_MAP_TBL1_TIMER_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_TIMER_SHIFT: u32 = 24;
pub const MSI_MAP_TBL1_TXQ1_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_TXQ1_SHIFT: u32 = 20;
pub const MSI_MAP_TBL1_TXQ0_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_TXQ0_SHIFT: u32 = 16;
pub const MSI_MAP_TBL1_RXQ3_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_RXQ3_SHIFT: u32 = 12;
pub const MSI_MAP_TBL1_RXQ2_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_RXQ2_SHIFT: u32 = 8;
pub const MSI_MAP_TBL1_RXQ1_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_RXQ1_SHIFT: u32 = 4;
pub const MSI_MAP_TBL1_RXQ0_MASK: u32 = 0xF;
pub const MSI_MAP_TBL1_RXQ0_SHIFT: u32 = 0;

pub const MSI_MAP_TBL2: u32 = 0x15D8;
pub const MSI_MAP_TBL2_PHY_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_PHY_SHIFT: u32 = 28;
pub const MSI_MAP_TBL2_SMB_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_SMB_SHIFT: u32 = 24;
pub const MSI_MAP_TBL2_TXQ3_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_TXQ3_SHIFT: u32 = 20;
pub const MSI_MAP_TBL2_TXQ2_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_TXQ2_SHIFT: u32 = 16;
pub const MSI_MAP_TBL2_RXQ7_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_RXQ7_SHIFT: u32 = 12;
pub const MSI_MAP_TBL2_RXQ6_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_RXQ6_SHIFT: u32 = 8;
pub const MSI_MAP_TBL2_RXQ5_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_RXQ5_SHIFT: u32 = 4;
pub const MSI_MAP_TBL2_RXQ4_MASK: u32 = 0xF;
pub const MSI_MAP_TBL2_RXQ4_SHIFT: u32 = 0;

pub const MSI_ID_MAP: u32 = 0x15D4;
pub const MSI_ID_MAP_RXQ7: u32 = 1 << 30;
pub const MSI_ID_MAP_RXQ6: u32 = 1 << 29;
pub const MSI_ID_MAP_RXQ5: u32 = 1 << 28;
pub const MSI_ID_MAP_RXQ4: u32 = 1 << 27;
/* bit26:: u32 = 0:common,1:timer */
pub const MSI_ID_MAP_PCIELNKDW: u32 = 1 << 26;
pub const MSI_ID_MAP_PCIECERR: u32 = 1 << 25;
pub const MSI_ID_MAP_PCIENFERR: u32 = 1 << 24;
pub const MSI_ID_MAP_PCIEFERR: u32 = 1 << 23;
pub const MSI_ID_MAP_PCIEUR: u32 = 1 << 22;
pub const MSI_ID_MAP_MACTX: u32 = 1 << 21;
pub const MSI_ID_MAP_MACRX: u32 = 1 << 20;
pub const MSI_ID_MAP_RXQ3: u32 = 1 << 19;
pub const MSI_ID_MAP_RXQ2: u32 = 1 << 18;
pub const MSI_ID_MAP_RXQ1: u32 = 1 << 17;
pub const MSI_ID_MAP_RXQ0: u32 = 1 << 16;
pub const MSI_ID_MAP_TXQ0: u32 = 1 << 15;
pub const MSI_ID_MAP_TXQTO: u32 = 1 << 14;
pub const MSI_ID_MAP_LPW: u32 = 1 << 13;
pub const MSI_ID_MAP_PHY: u32 = 1 << 12;
pub const MSI_ID_MAP_TXCREDIT: u32 = 1 << 11;
pub const MSI_ID_MAP_DMAW: u32 = 1 << 10;
pub const MSI_ID_MAP_DMAR: u32 = 1 << 9;
pub const MSI_ID_MAP_TXFUR: u32 = 1 << 8;
pub const MSI_ID_MAP_TXQ3: u32 = 1 << 7;
pub const MSI_ID_MAP_TXQ2: u32 = 1 << 6;
pub const MSI_ID_MAP_TXQ1: u32 = 1 << 5;
pub const MSI_ID_MAP_RFDUR: u32 = 1 << 4;
pub const MSI_ID_MAP_RXFOV: u32 = 1 << 3;
pub const MSI_ID_MAP_MANU: u32 = 1 << 2;
pub const MSI_ID_MAP_TIMER: u32 = 1 << 1;
pub const MSI_ID_MAP_SMB: u32 = 1 << 0;

pub const MSI_RETRANS_TIMER: u32 = 0x1920;
/* bit16:: u32 = 1:line,0:standard */
pub const MSI_MASK_SEL_LINE: u32 = 1 << 16;
pub const MSI_RETRANS_TM_MASK: u32 = 0xFFFF;
pub const MSI_RETRANS_TM_SHIFT: u32 = 0;

pub const CR_DMA_CTRL: u32 = 0x1930;
pub const CR_DMA_CTRL_PRI: u32 = 1 << 22;
pub const CR_DMA_CTRL_RRDRXD_JOINT: u32 = 1 << 21;
pub const CR_DMA_CTRL_BWCREDIT_MASK: u32 = 0x3;
pub const CR_DMA_CTRL_BWCREDIT_SHIFT: u32 = 19;
pub const CR_DMA_CTRL_BWCREDIT_2KB: u32 = 0;
pub const CR_DMA_CTRL_BWCREDIT_1KB: u32 = 1;
pub const CR_DMA_CTRL_BWCREDIT_4KB: u32 = 2;
pub const CR_DMA_CTRL_BWCREDIT_8KB: u32 = 3;
pub const CR_DMA_CTRL_BW_EN: u32 = 1 << 18;
pub const CR_DMA_CTRL_BW_RATIO_MASK: u32 = 0x3;
pub const CR_DMA_CTRL_BW_RATIO_1_2: u32 = 0;
pub const CR_DMA_CTRL_BW_RATIO_1_4: u32 = 1;
pub const CR_DMA_CTRL_BW_RATIO_1_8: u32 = 2;
pub const CR_DMA_CTRL_BW_RATIO_2_1: u32 = 3;
pub const CR_DMA_CTRL_SOFT_RST: u32 = 1 << 11;
pub const CR_DMA_CTRL_TXEARLY_EN: u32 = 1 << 10;
pub const CR_DMA_CTRL_RXEARLY_EN: u32 = 1 << 9;
pub const CR_DMA_CTRL_WEARLY_EN: u32 = 1 << 8;
pub const CR_DMA_CTRL_RXTH_MASK: u32 = 0xF;
pub const CR_DMA_CTRL_WTH_MASK: u32 = 0xF;

pub const EFUSE_BIST: u32 = 0x1934;
pub const EFUSE_BIST_COL_MASK: u32 = 0x3F;
pub const EFUSE_BIST_COL_SHIFT: u32 = 24;
pub const EFUSE_BIST_ROW_MASK: u32 = 0x7F;
pub const EFUSE_BIST_ROW_SHIFT: u32 = 12;
pub const EFUSE_BIST_STEP_MASK: u32 = 0xF;
pub const EFUSE_BIST_STEP_SHIFT: u32 = 8;
pub const EFUSE_BIST_PAT_MASK: u32 = 0x7;
pub const EFUSE_BIST_PAT_SHIFT: u32 = 4;
pub const EFUSE_BIST_CRITICAL: u32 = 1 << 3;
pub const EFUSE_BIST_FIXED: u32 = 1 << 2;
pub const EFUSE_BIST_FAIL: u32 = 1 << 1;
pub const EFUSE_BIST_NOW: u32 = 1 << 0;

/* CR DMA ctrl */

/* TX QoS */
pub const WRR: u32 = 0x1938;
pub const WRR_PRI_MASK: u32 = 0x3;
pub const WRR_PRI_SHIFT: u32 = 29;
pub const WRR_PRI_RESTRICT_ALL: u32 = 0;
pub const WRR_PRI_RESTRICT_HI: u32 = 1;
pub const WRR_PRI_RESTRICT_HI2: u32 = 2;
pub const WRR_PRI_RESTRICT_NONE: u32 = 3;
pub const WRR_PRI3_MASK: u32 = 0x1F;
pub const WRR_PRI3_SHIFT: u32 = 24;
pub const WRR_PRI2_MASK: u32 = 0x1F;
pub const WRR_PRI2_SHIFT: u32 = 16;
pub const WRR_PRI1_MASK: u32 = 0x1F;
pub const WRR_PRI1_SHIFT: u32 = 8;
pub const WRR_PRI0_MASK: u32 = 0x1F;
pub const WRR_PRI0_SHIFT: u32 = 0;

pub const HQTPD: u32 = 0x193C;
pub const HQTPD_BURST_EN: u32 = 1 << 31;
pub const HQTPD_Q3_NUMPREF_MASK: u32 = 0xF;
pub const HQTPD_Q3_NUMPREF_SHIFT: u32 = 8;
pub const HQTPD_Q2_NUMPREF_MASK: u32 = 0xF;
pub const HQTPD_Q2_NUMPREF_SHIFT: u32 = 4;
pub const HQTPD_Q1_NUMPREF_MASK: u32 = 0xF;
pub const HQTPD_Q1_NUMPREF_SHIFT: u32 = 0;

pub const CPUMAP1: u32 = 0x19A0;
pub const CPUMAP1_VCT7_MASK: u32 = 0xF;
pub const CPUMAP1_VCT7_SHIFT: u32 = 28;
pub const CPUMAP1_VCT6_MASK: u32 = 0xF;
pub const CPUMAP1_VCT6_SHIFT: u32 = 24;
pub const CPUMAP1_VCT5_MASK: u32 = 0xF;
pub const CPUMAP1_VCT5_SHIFT: u32 = 20;
pub const CPUMAP1_VCT4_MASK: u32 = 0xF;
pub const CPUMAP1_VCT4_SHIFT: u32 = 16;
pub const CPUMAP1_VCT3_MASK: u32 = 0xF;
pub const CPUMAP1_VCT3_SHIFT: u32 = 12;
pub const CPUMAP1_VCT2_MASK: u32 = 0xF;
pub const CPUMAP1_VCT2_SHIFT: u32 = 8;
pub const CPUMAP1_VCT1_MASK: u32 = 0xF;
pub const CPUMAP1_VCT1_SHIFT: u32 = 4;
pub const CPUMAP1_VCT0_MASK: u32 = 0xF;
pub const CPUMAP1_VCT0_SHIFT: u32 = 0;

pub const CPUMAP2: u32 = 0x19A4;
pub const CPUMAP2_VCT15_MASK: u32 = 0xF;
pub const CPUMAP2_VCT15_SHIFT: u32 = 28;
pub const CPUMAP2_VCT14_MASK: u32 = 0xF;
pub const CPUMAP2_VCT14_SHIFT: u32 = 24;
pub const CPUMAP2_VCT13_MASK: u32 = 0xF;
pub const CPUMAP2_VCT13_SHIFT: u32 = 20;
pub const CPUMAP2_VCT12_MASK: u32 = 0xF;
pub const CPUMAP2_VCT12_SHIFT: u32 = 16;
pub const CPUMAP2_VCT11_MASK: u32 = 0xF;
pub const CPUMAP2_VCT11_SHIFT: u32 = 12;
pub const CPUMAP2_VCT10_MASK: u32 = 0xF;
pub const CPUMAP2_VCT10_SHIFT: u32 = 8;
pub const CPUMAP2_VCT9_MASK: u32 = 0xF;
pub const CPUMAP2_VCT9_SHIFT: u32 = 4;
pub const CPUMAP2_VCT8_MASK: u32 = 0xF;
pub const CPUMAP2_VCT8_SHIFT: u32 = 0;

pub const MISC: u32 = 0x19C0;
/* bit31:: u32 = 0:vector,1:cpu */
pub const MISC_MODU: u32 = 1 << 31;
pub const MISC_OVERCUR: u32 = 1 << 29;
pub const MISC_PSWR_EN: u32 = 1 << 28;
pub const MISC_PSW_CTRL_MASK: u32 = 0xF;
pub const MISC_PSW_CTRL_SHIFT: u32 = 24;
pub const MISC_PSW_OCP_MASK: u32 = 0x7;
pub const MISC_PSW_OCP_SHIFT: u32 = 21;
pub const MISC_PSW_OCP_DEF: u32 = 0x7;
pub const MISC_V18_HIGH: u32 = 1 << 20;
pub const MISC_LPO_CTRL_MASK: u32 = 0xF;
pub const MISC_LPO_CTRL_SHIFT: u32 = 16;
pub const MISC_ISO_EN: u32 = 1 << 12;
pub const MISC_XSTANA_ALWAYS_ON: u32 = 1 << 11;
pub const MISC_SYS25M_SEL_ADAPTIVE: u32 = 1 << 10;
pub const MISC_SPEED_SIM: u32 = 1 << 9;
pub const MISC_S1_LWP_EN: u32 = 1 << 8;
/* bit7: pcie/mac do pwsaving as phy in lpw state */
pub const MISC_MACLPW: u32 = 1 << 7;
pub const MISC_125M_SW: u32 = 1 << 6;
pub const MISC_INTNLOSC_OFF_EN: u32 = 1 << 5;
/* bit4:: u32 = 0:chipset,1:crystle */
pub const MISC_EXTN25M_SEL: u32 = 1 << 4;
pub const MISC_INTNLOSC_OPEN: u32 = 1 << 3;
pub const MISC_SMBUS_AT_LED: u32 = 1 << 2;
pub const MISC_PPS_AT_LED_MASK: u32 = 0x3;
pub const MISC_PPS_AT_LED_SHIFT: u32 = 0;
pub const MISC_PPS_AT_LED_ACT: u32 = 1;
pub const MISC_PPS_AT_LED_10_100: u32 = 2;
pub const MISC_PPS_AT_LED_1000: u32 = 3;

pub const MISC1: u32 = 0x19C4;
pub const MSC1_BLK_CRASPM_REQ: u32 = 1 << 15;

pub const MSIC2: u32 = 0x19C8;
pub const MSIC2_CALB_START: u32 = 1 << 0;

pub const MISC3: u32 = 0x19CC;
/* bit1:: u32 = 1:Software control: u32 = 25M */
pub const MISC3_25M_BY_SW: u32 = 1 << 1;
/* bit0:: u32 = 25M switch to intnl OSC */
pub const MISC3_25M_NOTO_INTNL: u32 = 1 << 0;

/* MSIX tbl in memory space */
pub const MSIX_ENTRY_BASE: u32 = 0x2000;

/***************************** IO mapping registers ***************************/
pub const IO_ADDR: u32 = 0x00;
pub const IO_DATA: u32 = 0x04;
/* same as reg1400 */
pub const IO_MASTER: u32 = 0x08;
/* same as reg1480 */
pub const IO_MAC_CTRL: u32 = 0x0C;
/* same as reg1600 */
pub const IO_ISR: u32 = 0x10;
/* same as reg: u32 = 1604 */
pub const IO_IMR: u32 = 0x14;
/* word, same as reg15F0 */
pub const IO_TPD_PRI1_PIDX: u32 = 0x18;
/* word, same as reg15F2 */
pub const IO_TPD_PRI0_PIDX: u32 = 0x1A;
/* word, same as reg15F4 */
pub const IO_TPD_PRI1_CIDX: u32 = 0x1C;
/* word, same as reg15F6 */
pub const IO_TPD_PRI0_CIDX: u32 = 0x1E;
/* word, same as reg15E0 */
pub const IO_RFD_PIDX: u32 = 0x20;
/* word, same as reg15F8 */
pub const IO_RFD_CIDX: u32 = 0x30;
/* same as reg1414 */
pub const IO_MDIO: u32 = 0x38;
/* same as reg140C */
pub const IO_PHY_CTRL: u32 = 0x3C;

/********************* PHY regs definition ***************************/

/* Autoneg Advertisement Register */
pub const ADVERTISE_SPEED_MASK: u16 = 0x01E0;
pub const ADVERTISE_DEFAULT_CAP: u16 = 0x1DE0;

/* 1000BASE-T Control Register (0x9) */
pub const GIGA_CR_1000T_HD_CAPS: u16 = 0x0100;
pub const GIGA_CR_1000T_FD_CAPS: u16 = 0x0200;
pub const GIGA_CR_1000T_REPEATER_DTE: u16 = 0x0400;

pub const GIGA_CR_1000T_MS_VALUE: u16 = 0x0800;

pub const GIGA_CR_1000T_MS_ENABLE: u16 = 0x1000;

pub const GIGA_CR_1000T_TEST_MODE_NORMAL: u16 = 0x0000;
pub const GIGA_CR_1000T_TEST_MODE_1: u16 = 0x2000;
pub const GIGA_CR_1000T_TEST_MODE_2: u16 = 0x4000;
pub const GIGA_CR_1000T_TEST_MODE_3: u16 = 0x6000;
pub const GIGA_CR_1000T_TEST_MODE_4: u16 = 0x8000;
pub const GIGA_CR_1000T_SPEED_MASK: u16 = 0x0300;
pub const GIGA_CR_1000T_DEFAULT_CAP: u16 = 0x0300;

/* 1000BASE-T Status Register */
pub const MII_GIGA_SR: u16 = 0x0A;

/* PHY Specific Status Register */
pub const MII_GIGA_PSSR: u16 = 0x11;
pub const GIGA_PSSR_FC_RXEN: u16 = 0x0004;
pub const GIGA_PSSR_FC_TXEN: u16 = 0x0008;
pub const GIGA_PSSR_SPD_DPLX_RESOLVED: u16 = 0x0800;
pub const GIGA_PSSR_DPLX: u16 = 0x2000;
pub const GIGA_PSSR_SPEED: u16 = 0xC000;
pub const GIGA_PSSR_10MBS: u16 = 0x0000;
pub const GIGA_PSSR_100MBS: u16 = 0x4000;
pub const GIGA_PSSR_1000MBS: u16 = 0x8000;

/* PHY Interrupt Enable Register */
pub const MII_IER: u16 = 0x12;
pub const IER_LINK_UP: u16 = 0x0400;
pub const IER_LINK_DOWN: u16 = 0x0800;

/* PHY Interrupt Status Register */
pub const MII_ISR: u16 = 0x13;
pub const ISR_LINK_UP: u16 = 0x0400;
pub const ISR_LINK_DOWN: u16 = 0x0800;

/* Cable-Detect-Test Control Register */
pub const MII_CDTC: u16 = 0x16;
/* self clear */
pub const CDTC_EN: u16 = 1;
pub const CDTC_PAIR_MASK: u16 = 0x3;
pub const CDTC_PAIR_SHIFT: u16 = 8;

/* Cable-Detect-Test Status Register */
pub const MII_CDTS: u16 = 0x1C;
pub const CDTS_STATUS_MASK: u16 = 0x3;
pub const CDTS_STATUS_SHIFT: u16 = 8;
pub const CDTS_STATUS_NORMAL: u16 = 0;
pub const CDTS_STATUS_SHORT: u16 = 1;
pub const CDTS_STATUS_OPEN: u16 = 2;
pub const CDTS_STATUS_INVALID: u16 = 3;

pub const MII_DBG_ADDR: u16 = 0x1D;
pub const MII_DBG_DATA: u16 = 0x1E;

/***************************** debug port *************************************/

pub const MIIDBG_ANACTRL: u16 = 0x00;
pub const ANACTRL_CLK125M_DELAY_EN: u16 = 0x8000;
pub const ANACTRL_VCO_FAST: u16 = 0x4000;
pub const ANACTRL_VCO_SLOW: u16 = 0x2000;
pub const ANACTRL_AFE_MODE_EN: u16 = 0x1000;
pub const ANACTRL_LCKDET_PHY: u16 = 0x0800;
pub const ANACTRL_LCKDET_EN: u16 = 0x0400;
pub const ANACTRL_OEN_125M: u16 = 0x0200;
pub const ANACTRL_HBIAS_EN: u16 = 0x0100;
pub const ANACTRL_HB_EN: u16 = 0x0080;
pub const ANACTRL_SEL_HSP: u16 = 0x0040;
pub const ANACTRL_CLASSA_EN: u16 = 0x0020;
pub const ANACTRL_MANUSWON_SWR_MASK: u16 = 0x3;
pub const ANACTRL_MANUSWON_SWR_SHIFT: u16 = 2;
pub const ANACTRL_MANUSWON_SWR_2V: u16 = 0;
pub const ANACTRL_MANUSWON_SWR_1P9V: u16 = 1;
pub const ANACTRL_MANUSWON_SWR_1P8V: u16 = 2;
pub const ANACTRL_MANUSWON_SWR_1P7V: u16 = 3;
pub const ANACTRL_MANUSWON_BW3_4M: u16 = 0x0002;
pub const ANACTRL_RESTART_CAL: u16 = 0x0001;
pub const ANACTRL_DEF: u16 = 0x02EF;

pub const MIIDBG_SYSMODCTRL: u16 = 0x04;
pub const SYSMODCTRL_IECHOADJ_PFMH_PHY: u16 = 0x8000;
pub const SYSMODCTRL_IECHOADJ_BIASGEN: u16 = 0x4000;
pub const SYSMODCTRL_IECHOADJ_PFML_PHY: u16 = 0x2000;
pub const SYSMODCTRL_IECHOADJ_PS_MASK: u16 = 0x3;
pub const SYSMODCTRL_IECHOADJ_PS_SHIFT: u16 = 10;
pub const SYSMODCTRL_IECHOADJ_PS_40: u16 = 3;
pub const SYSMODCTRL_IECHOADJ_PS_20: u16 = 2;
pub const SYSMODCTRL_IECHOADJ_PS_0: u16 = 1;
pub const SYSMODCTRL_IECHOADJ_10BT_100MV: u16 = 0x0040;
pub const SYSMODCTRL_IECHOADJ_HLFAP_MASK: u16 = 0x3;
pub const SYSMODCTRL_IECHOADJ_HLFAP_SHIFT: u16 = 4;
pub const SYSMODCTRL_IECHOADJ_VDFULBW: u16 = 0x0008;
pub const SYSMODCTRL_IECHOADJ_VDBIASHLF: u16 = 0x0004;
pub const SYSMODCTRL_IECHOADJ_VDAMPHLF: u16 = 0x0002;
pub const SYSMODCTRL_IECHOADJ_VDLANSW: u16 = 0x0001;
/* en half bias */
pub const SYSMODCTRL_IECHOADJ_DEF: u16 = 0xBB8B;

pub const MIIDBG_SRDSYSMOD: u16 = 0x05;
pub const SRDSYSMOD_LCKDET_EN: u16 = 0x2000;
pub const SRDSYSMOD_PLL_EN: u16 = 0x0800;
pub const SRDSYSMOD_SEL_HSP: u16 = 0x0400;
pub const SRDSYSMOD_HLFTXDR: u16 = 0x0200;
pub const SRDSYSMOD_TXCLK_DELAY_EN: u16 = 0x0100;
pub const SRDSYSMOD_TXELECIDLE: u16 = 0x0080;
pub const SRDSYSMOD_DEEMP_EN: u16 = 0x0040;
pub const SRDSYSMOD_MS_PAD: u16 = 0x0004;
pub const SRDSYSMOD_CDR_ADC_VLTG: u16 = 0x0002;
pub const SRDSYSMOD_CDR_DAC_1MA: u16 = 0x0001;
pub const SRDSYSMOD_DEF: u16 = 0x2C46;

pub const MIIDBG_HIBNEG: u16 = 0x0B;
pub const HIBNEG_PSHIB_EN: u16 = 0x8000;
pub const HIBNEG_WAKE_BOTH: u16 = 0x4000;
pub const HIBNEG_ONOFF_ANACHG_SUDEN: u16 = 0x2000;
pub const HIBNEG_HIB_PULSE: u16 = 0x1000;
pub const HIBNEG_GATE_25M_EN: u16 = 0x0800;
pub const HIBNEG_RST_80U: u16 = 0x0400;
pub const HIBNEG_RST_TIMER_MASK: u16 = 0x3;
pub const HIBNEG_RST_TIMER_SHIFT: u16 = 8;
pub const HIBNEG_GTX_CLK_DELAY_MASK: u16 = 0x3;
pub const HIBNEG_GTX_CLK_DELAY_SHIFT: u16 = 5;
pub const HIBNEG_BYPSS_BRKTIMER: u16 = 0x0010;
pub const HIBNEG_DEF: u16 = 0xBC40;
pub const HIBNEG_NOHIB: u16 = HIBNEG_DEF & !(HIBNEG_PSHIB_EN | HIBNEG_HIB_PULSE);

pub const MIIDBG_TST10BTCFG: u16 = 0x12;
pub const TST10BTCFG_INTV_TIMER_MASK: u16 = 0x3;
pub const TST10BTCFG_INTV_TIMER_SHIFT: u16 = 14;
pub const TST10BTCFG_TRIGER_TIMER_MASK: u16 = 0x3;
pub const TST10BTCFG_TRIGER_TIMER_SHIFT: u16 = 12;
pub const TST10BTCFG_DIV_MAN_MLT3_EN: u16 = 0x0800;
pub const TST10BTCFG_OFF_DAC_IDLE: u16 = 0x0400;
pub const TST10BTCFG_LPBK_DEEP: u16 = 0x0004;
pub const TST10BTCFG_DEF: u16 = 0x4C04;

pub const MIIDBG_AZ_ANADECT: u16 = 0x15;
pub const AZ_ANADECT_10BTRX_TH: u16 = 0x8000;
pub const AZ_ANADECT_BOTH_01CHNL: u16 = 0x4000;
pub const AZ_ANADECT_INTV_MASK: u16 = 0x3F;
pub const AZ_ANADECT_INTV_SHIFT: u16 = 8;
pub const AZ_ANADECT_THRESH_MASK: u16 = 0xF;
pub const AZ_ANADECT_THRESH_SHIFT: u16 = 4;
pub const AZ_ANADECT_CHNL_MASK: u16 = 0xF;
pub const AZ_ANADECT_CHNL_SHIFT: u16 = 0;
pub const AZ_ANADECT_DEF: u16 = 0x3220;
pub const AZ_ANADECT_LONG: u16 = 0x3210;

pub const MIIDBG_MSE16DB: u16 = 0x18;
pub const MSE16DB_UP: u16 = 0x05EA;
pub const MSE16DB_DOWN: u16 = 0x02EA;

pub const MIIDBG_MSE20DB: u16 = 0x1C;
pub const MSE20DB_TH_MASK: u16 = 0x7F;
pub const MSE20DB_TH_SHIFT: u16 = 2;
pub const MSE20DB_TH_DEF: u16 = 0x2E;
pub const MSE20DB_TH_HI: u16 = 0x54;

pub const MIIDBG_AGC: u16 = 0x23;
pub const AGC_2_VGA_MASK: u16 = 0x3F;
pub const AGC_2_VGA_SHIFT: u16 = 8;
pub const AGC_LONG1G_LIMT: u16 = 40;
pub const AGC_LONG100M_LIMT: u16 = 44;

pub const MIIDBG_LEGCYPS: u16 = 0x29;
pub const LEGCYPS_EN: u16 = 0x8000;
pub const LEGCYPS_DAC_AMP1000_MASK: u16 = 0x7;
pub const LEGCYPS_DAC_AMP1000_SHIFT: u16 = 12;
pub const LEGCYPS_DAC_AMP100_MASK: u16 = 0x7;
pub const LEGCYPS_DAC_AMP100_SHIFT: u16 = 9;
pub const LEGCYPS_DAC_AMP10_MASK: u16 = 0x7;
pub const LEGCYPS_DAC_AMP10_SHIFT: u16 = 6;
pub const LEGCYPS_UNPLUG_TIMER_MASK: u16 = 0x7;
pub const LEGCYPS_UNPLUG_TIMER_SHIFT: u16 = 3;
pub const LEGCYPS_UNPLUG_DECT_EN: u16 = 0x0004;
pub const LEGCYPS_ECNC_PS_EN: u16 = 0x0001;
pub const LEGCYPS_DEF: u16 = 0x129D;

pub const MIIDBG_TST100BTCFG: u16 = 0x36;
pub const TST100BTCFG_NORMAL_BW_EN: u16 = 0x8000;
pub const TST100BTCFG_BADLNK_BYPASS: u16 = 0x4000;
pub const TST100BTCFG_SHORTCABL_TH_MASK: u16 = 0x3F;
pub const TST100BTCFG_SHORTCABL_TH_SHIFT: u16 = 8;
pub const TST100BTCFG_LITCH_EN: u16 = 0x0080;
pub const TST100BTCFG_VLT_SW: u16 = 0x0040;
pub const TST100BTCFG_LONGCABL_TH_MASK: u16 = 0x3F;
pub const TST100BTCFG_LONGCABL_TH_SHIFT: u16 = 0;
pub const TST100BTCFG_DEF: u16 = 0xE12C;

pub const MIIDBG_GREENCFG: u16 = 0x3B;
pub const GREENCFG_MSTPS_MSETH2_MASK: u16 = 0xFF;
pub const GREENCFG_MSTPS_MSETH2_SHIFT: u16 = 8;
pub const GREENCFG_MSTPS_MSETH1_MASK: u16 = 0xFF;
pub const GREENCFG_MSTPS_MSETH1_SHIFT: u16 = 0;
pub const GREENCFG_DEF: u16 = 0x7078;

pub const MIIDBG_GREENCFG2: u16 = 0x3D;
pub const GREENCFG2_BP_GREEN: u16 = 0x8000;
pub const GREENCFG2_GATE_DFSE_EN: u16 = 0x0080;

/***************************** extension **************************************/

/******* dev 3 *********/
pub const MIIEXT_PCS: u8 = 3;

pub const MIIEXT_CLDCTRL3: u16 = 0x8003;
pub const CLDCTRL3_BP_CABLE1TH_DET_GT: u16 = 0x8000;
pub const CLDCTRL3_AZ_DISAMP: u16 = 0x1000;

pub const MIIEXT_CLDCTRL5: u16 = 0x8005;
pub const CLDCTRL5_BP_VD_HLFBIAS: u16 = 0x4000;

pub const MIIEXT_CLDCTRL6: u16 = 0x8006;
pub const CLDCTRL6_CAB_LEN_MASK: u16 = 0xFF;
pub const CLDCTRL6_CAB_LEN_SHIFT: u16 = 0;
pub const CLDCTRL6_CAB_LEN_SHORT1G: u16 = 116;
pub const CLDCTRL6_CAB_LEN_SHORT100M: u16 = 152;

pub const MIIEXT_CLDCTRL7: u16 = 0x8007;
pub const CLDCTRL7_VDHLF_BIAS_TH_MASK: u16 = 0x7F;
pub const CLDCTRL7_VDHLF_BIAS_TH_SHIFT: u16 = 9;
pub const CLDCTRL7_AFE_AZ_MASK: u16 = 0x1F;
pub const CLDCTRL7_AFE_AZ_SHIFT: u16 = 4;
pub const CLDCTRL7_SIDE_PEAK_TH_MASK: u16 = 0xF;
pub const CLDCTRL7_SIDE_PEAK_TH_SHIFT: u16 = 0;
pub const CLDCTRL7_DEF: u16 = 0x6BF6;

pub const MIIEXT_AZCTRL: u16 = 0x8008;
pub const AZCTRL_SHORT_TH_MASK: u16 = 0xFF;
pub const AZCTRL_SHORT_TH_SHIFT: u16 = 8;
pub const AZCTRL_LONG_TH_MASK: u16 = 0xFF;
pub const AZCTRL_LONG_TH_SHIFT: u16 = 0;
pub const AZCTRL_DEF: u16 = 0x1629;

pub const MIIEXT_AZCTRL2: u16 = 0x8009;
pub const AZCTRL2_WAKETRNING_MASK: u16 = 0xFF;
pub const AZCTRL2_WAKETRNING_SHIFT: u16 = 8;
pub const AZCTRL2_QUIET_TIMER_MASK: u16 = 0x3;
pub const AZCTRL2_QUIET_TIMER_SHIFT: u16 = 6;
pub const AZCTRL2_PHAS_JMP2: u16 = 0x0010;
pub const AZCTRL2_CLKTRCV_125MD16: u16 = 0x0008;
pub const AZCTRL2_GATE1000_EN: u16 = 0x0004;
pub const AZCTRL2_AVRG_FREQ: u16 = 0x0002;
pub const AZCTRL2_PHAS_JMP4: u16 = 0x0001;
pub const AZCTRL2_DEF: u16 = 0x32C0;

pub const MIIEXT_AZCTRL6: u16 = 0x800D;

pub const MIIEXT_VDRVBIAS: u16 = 0x8062;
pub const VDRVBIAS_SEL_MASK: u16 = 0x3;
pub const VDRVBIAS_SEL_SHIFT: u16 = 0;
pub const VDRVBIAS_DEF: u16 = 0x3;

/********* dev 7 **********/
pub const MIIEXT_ANEG: u8 = 7;

pub const MIIEXT_LOCAL_EEEADV: u16 = 0x3C;
pub const LOCAL_EEEADV_1000BT: u16 = 0x0004;
pub const LOCAL_EEEADV_100BT: u16 = 0x0002;

pub const MIIEXT_REMOTE_EEEADV: u16 = 0x3D;
pub const REMOTE_EEEADV_1000BT: u16 = 0x0004;
pub const REMOTE_EEEADV_100BT: u16 = 0x0002;

pub const MIIEXT_EEE_ANEG: u16 = 0x8000;
pub const EEE_ANEG_1000M: u16 = 0x0004;
pub const EEE_ANEG_100M: u16 = 0x0002;

pub const MIIEXT_AFE: u16 = 0x801A;
pub const AFE_10BT_100M_TH: u16 = 0x0040;

pub const MIIEXT_S3DIG10: u16 = 0x8023;
/* bit0:: u16 = 1:bypass: u16 = 10BT rx fifo,: u16 = 0:riginal: u16 = 10BT rx */
pub const MIIEXT_S3DIG10_SL: u16 = 0x0001;
pub const MIIEXT_S3DIG10_DEF: u16 = 0;

pub const MIIEXT_NLP34: u16 = 0x8025;
/* for: u16 = 160m */
pub const MIIEXT_NLP34_DEF: u16 = 0x1010;

pub const MIIEXT_NLP56: u16 = 0x8026;
/* for: u16 = 160m */
pub const MIIEXT_NLP56_DEF: u16 = 0x1010;

pub const MIIEXT_NLP78: u16 = 0x8027;
/* for: u16 = 160m */
pub const MIIEXT_NLP78_160M_DEF: u16 = 0x8D05;
pub const MIIEXT_NLP78_120M_DEF: u16 = 0x8A05;
