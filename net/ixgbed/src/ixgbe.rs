#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(clippy::unreadable_literal)]

pub const IXGBE_EIMC: u32                       = 0x00888;

pub const IXGBE_CTRL: u32                       = 0x00000;
pub const IXGBE_CTRL_LNK_RST: u32               = 0x00000008; /* Link Reset. Resets everything. */
pub const IXGBE_CTRL_RST: u32                   = 0x04000000; /* Reset (SW) */
pub const IXGBE_CTRL_RST_MASK: u32              = IXGBE_CTRL_LNK_RST | IXGBE_CTRL_RST;

pub const IXGBE_EEC: u32                        = 0x10010;
pub const IXGBE_EEC_ARD: u32                    = 0x00000200; /* EEPROM Auto Read Done */

pub const IXGBE_RDRXCTL: u32                    = 0x02F00;
pub const IXGBE_RDRXCTL_RESERVED_BITS: u32      = 1 << 25 | 1 << 26;
pub const IXGBE_RDRXCTL_DMAIDONE: u32           = 0x00000008; /* DMA init cycle done */

pub const IXGBE_AUTOC: u32                      = 0x042A0;
pub const IXGBE_AUTOC_LMS_SHIFT: u32            = 13;
pub const IXGBE_AUTOC_LMS_MASK: u32             = 0x7 << IXGBE_AUTOC_LMS_SHIFT;
pub const IXGBE_AUTOC_LMS_10G_SERIAL: u32       = 0x3 << IXGBE_AUTOC_LMS_SHIFT;
pub const IXGBE_AUTOC_10G_PMA_PMD_MASK: u32     = 0x00000180;
pub const IXGBE_AUTOC_10G_PMA_PMD_SHIFT: u32    = 7;
pub const IXGBE_AUTOC_10G_XAUI: u32             = 0x0 << IXGBE_AUTOC_10G_PMA_PMD_SHIFT;
pub const IXGBE_AUTOC_AN_RESTART: u32           = 0x00001000;

pub const IXGBE_GPRC: u32                       = 0x04074;
pub const IXGBE_GPTC: u32                       = 0x04080;
pub const IXGBE_GORCL: u32                      = 0x04088;
pub const IXGBE_GORCH: u32                      = 0x0408C;
pub const IXGBE_GOTCL: u32                      = 0x04090;
pub const IXGBE_GOTCH: u32                      = 0x04094;

pub const IXGBE_RXCTRL: u32                     = 0x03000;
pub const IXGBE_RXCTRL_RXEN: u32                = 0x00000001; /* Enable Receiver */

pub fn IXGBE_RXPBSIZE(i: u32) -> u32 {
    0x03C00 + (i * 4)
}

pub const IXGBE_RXPBSIZE_128KB: u32             = 0x00020000; /* 128KB Packet Buffer */
pub const IXGBE_HLREG0: u32                     = 0x04240;
pub const IXGBE_HLREG0_RXCRCSTRP: u32           = 0x00000002; /* bit  1 */
pub const IXGBE_RDRXCTL_CRCSTRIP: u32           = 0x00000002; /* CRC Strip */

pub const IXGBE_FCTRL: u32                      = 0x05080;
pub const IXGBE_FCTRL_BAM: u32                  = 0x00000400; /* Broadcast Accept Mode */

pub fn IXGBE_SRRCTL(i: u32) -> u32 {
    if i <= 15 {
        0x02100 + (i * 4)
    } else if i < 64 {
        0x01014 + (i * 0x40)
    } else {
        0x0D014 + ((i - 64) * 0x40)
    }
}

pub const IXGBE_SRRCTL_DESCTYPE_MASK: u32       = 0x0E000000;
pub const IXGBE_SRRCTL_DESCTYPE_ADV_ONEBUF: u32 = 0x02000000;
pub const IXGBE_SRRCTL_DROP_EN: u32             = 0x10000000;

pub fn IXGBE_RDBAL(i: u32) -> u32 {
    if i < 64 {
        0x01000 + (i * 0x40)
    } else {
        0x0D000 + ((i - 64) * 0x40)
    }
}
pub fn IXGBE_RDBAH(i: u32) -> u32 {
    if i < 64 {
        0x01004 + (i * 0x40)
    } else {
        0x0D004 + ((i - 64) * 0x40)
    }
}
pub fn IXGBE_RDLEN(i: u32) -> u32 {
    if i < 64 {
        0x01008 + (i * 0x40)
    } else {
        0x0D008 + ((i - 64) * 0x40)
    }
}
pub fn IXGBE_RDH(i: u32) -> u32 {
    if i < 64 {
        0x01010 + (i * 0x40)
    } else {
        0x0D010 + ((i - 64) * 0x40)
    }
}
pub fn IXGBE_RDT(i: u32) -> u32 {
    if i < 64 {
        0x01018 + (i * 0x40)
    } else {
        0x0D018 + ((i - 64) * 0x40)
    }
}

pub const IXGBE_CTRL_EXT: u32                   = 0x00018;
pub const IXGBE_CTRL_EXT_NS_DIS: u32            = 0x00010000; /* No Snoop disable */

pub fn IXGBE_DCA_RXCTRL(i: u32) -> u32 {
    if i <= 15 {
        0x02200 + (i * 4)
    } else if i < 64 {
        0x0100C + (i * 0x40)
    } else {
        0x0D00C + ((i - 64) * 0x40)
    }
}

pub const IXGBE_HLREG0_TXCRCEN: u32             = 0x00000001; /* bit  0 */
pub const IXGBE_HLREG0_TXPADEN: u32             = 0x00000400; /* bit 10 */

pub fn IXGBE_TXPBSIZE(i: u32) -> u32 {
    0x0CC00 + (i * 4)
} /* 8 of these */

pub const IXGBE_TXPBSIZE_40KB: u32              = 0x0000A000; /* 40KB Packet Buffer */
pub const IXGBE_DTXMXSZRQ: u32                  = 0x08100;
pub const IXGBE_RTTDCS: u32                     = 0x04900;
pub const IXGBE_RTTDCS_ARBDIS: u32              = 0x00000040; /* DCB arbiter disable */

pub fn IXGBE_TDBAL(i: u32) -> u32 {
    0x06000 + (i * 0x40)
} /* 32 of them (0-31)*/
pub fn IXGBE_TDBAH(i: u32) -> u32 {
    0x06004 + (i * 0x40)
}
pub fn IXGBE_TDLEN(i: u32) -> u32 {
    0x06008 + (i * 0x40)
}
pub fn IXGBE_TXDCTL(i: u32) -> u32 {
    0x06028 + (i * 0x40)
}

pub const IXGBE_DMATXCTL: u32                   = 0x04A80;
pub const IXGBE_DMATXCTL_TE: u32                = 0x1; /* Transmit Enable */

pub fn IXGBE_RXDCTL(i: u32) -> u32 {
    if i < 64 {
        0x01028 + (i * 0x40)
    } else {
        0x0D028 + ((i - 64) * 0x40)
    }
}
pub const IXGBE_RXDCTL_ENABLE: u32              = 0x02000000; /* Ena specific Rx Queue */
pub const IXGBE_TXDCTL_ENABLE: u32              = 0x02000000; /* Ena specific Tx Queue */

pub fn IXGBE_TDH(i: u32) -> u32 {
    0x06010 + (i * 0x40)
}
pub fn IXGBE_TDT(i: u32) -> u32 {
    0x06018 + (i * 0x40)
}

pub const IXGBE_FCTRL_MPE: u32                  = 0x00000100; /* Multicast Promiscuous Ena*/
pub const IXGBE_FCTRL_UPE: u32                  = 0x00000200; /* Unicast Promiscuous Ena */

pub const IXGBE_LINKS: u32                      = 0x042A4;
pub const IXGBE_LINKS_UP: u32                   = 0x40000000;
pub const IXGBE_LINKS_SPEED_82599: u32          = 0x30000000;
pub const IXGBE_LINKS_SPEED_100_82599: u32      = 0x10000000;
pub const IXGBE_LINKS_SPEED_1G_82599: u32       = 0x20000000;
pub const IXGBE_LINKS_SPEED_10G_82599: u32      = 0x30000000;

pub fn IXGBE_RAL(i: u32) -> u32 {
    if i <= 15 {
        0x05400 + (i * 8)
    } else {
        0x0A200 + (i * 8)
    }
}

pub fn IXGBE_RAH(i: u32) -> u32 {
    if i <= 15 {
        0x05404 + (i * 8)
    } else {
        0x0A204 + (i * 8)
    }
}

pub const IXGBE_RXD_STAT_DD: u32                = 0x01; /* Descriptor Done */
pub const IXGBE_RXD_STAT_EOP: u32               = 0x02; /* End of Packet */
pub const IXGBE_RXDADV_STAT_DD: u32             = IXGBE_RXD_STAT_DD; /* Done */
pub const IXGBE_RXDADV_STAT_EOP: u32            = IXGBE_RXD_STAT_EOP; /* End of Packet */

pub const IXGBE_ADVTXD_PAYLEN_SHIFT: u32        = 14; /* Adv desc PAYLEN shift */
pub const IXGBE_TXD_CMD_EOP: u32                = 0x01000000; /* End of Packet */
pub const IXGBE_ADVTXD_DCMD_EOP: u32            = IXGBE_TXD_CMD_EOP; /* End of Packet */
pub const IXGBE_TXD_CMD_RS: u32                 = 0x08000000; /* Report Status */
pub const IXGBE_ADVTXD_DCMD_RS: u32             = IXGBE_TXD_CMD_RS; /* Report Status */
pub const IXGBE_TXD_CMD_IFCS: u32               = 0x02000000; /* Insert FCS (Ethernet CRC) */
pub const IXGBE_ADVTXD_DCMD_IFCS: u32           = IXGBE_TXD_CMD_IFCS; /* Insert FCS */
pub const IXGBE_TXD_CMD_DEXT: u32               = 0x20000000; /* Desc extension (0 = legacy) */
pub const IXGBE_ADVTXD_DTYP_DATA: u32           = 0x00300000; /* Adv Data Descriptor */
pub const IXGBE_ADVTXD_DCMD_DEXT: u32           = IXGBE_TXD_CMD_DEXT; /* Desc ext 1=Adv */
pub const IXGBE_TXD_STAT_DD: u32                = 0x00000001; /* Descriptor Done */
pub const IXGBE_ADVTXD_STAT_DD: u32             = IXGBE_TXD_STAT_DD; /* Descriptor Done */

/* Interrupt Registers */
pub const IXGBE_EICR: u32                       = 0x00800;
pub const IXGBE_EIAC: u32                       = 0x00810;
pub const IXGBE_EIMS: u32                       = 0x00880;
pub const IXGBE_IVAR_ALLOC_VAL: u32             = 0x80; /* Interrupt Allocation valid */
pub const IXGBE_EICR_RTX_QUEUE: u32             = 0x0000FFFF; /* RTx Queue Interrupt */

pub fn IXGBE_IVAR(i: u32) -> u32 {
    0x00900 + (i * 4)
} /* 24 at 0x900-0x960 */

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_read {
    pub pkt_addr: u64,
    /* Packet buffer address */
    pub hdr_addr: u64,
    /* Header buffer address */
}

/* Receive Descriptor - Advanced */
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_wb_lower_lo_dword_hs_rss {
    pub pkt_info: u16,
    /* RSS, Pkt type */
    pub hdr_info: u16,
    /* Splithdr, hdrlen */
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub union ixgbe_adv_rx_desc_wb_lower_lo_dword {
    pub data: u32,
    pub hs_rss: ixgbe_adv_rx_desc_wb_lower_lo_dword_hs_rss,
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_wb_lower_hi_dword_csum_ip {
    pub ip_id: u16,
    /* IP id */
    pub csum: u16,
    /* Packet Checksum */
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub union ixgbe_adv_rx_desc_wb_lower_hi_dword {
    pub rss: u32,
    /* RSS Hash */
    pub csum_ip: ixgbe_adv_rx_desc_wb_lower_hi_dword_csum_ip,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_wb_lower {
    pub lo_dword: ixgbe_adv_rx_desc_wb_lower_lo_dword,
    pub hi_dword: ixgbe_adv_rx_desc_wb_lower_hi_dword,
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_wb_upper {
    pub status_error: u32,
    /* ext status/error */
    pub length: u16,
    /* Packet length */
    pub vlan: u16,
    /* VLAN tag */
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_rx_desc_wb {
    pub lower: ixgbe_adv_rx_desc_wb_lower,
    pub upper: ixgbe_adv_rx_desc_wb_upper,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub union ixgbe_adv_rx_desc {
    pub read: ixgbe_adv_rx_desc_read,
    pub wb: ixgbe_adv_rx_desc_wb, /* writeback */
    _union_align: [u64; 2],
}

/* Transmit Descriptor - Advanced */
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_tx_desc_read {
    pub buffer_addr: u64,
    /* Address of descriptor's data buf */
    pub cmd_type_len: u32,
    pub olinfo_status: u32,
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct ixgbe_adv_tx_desc_wb {
    pub rsvd: u64,
    /* Reserved */
    pub nxtseq_seed: u32,
    pub status: u32,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub union ixgbe_adv_tx_desc {
    pub read: ixgbe_adv_tx_desc_read,
    pub wb: ixgbe_adv_tx_desc_wb,
    _union_align: [u64; 2],
}
