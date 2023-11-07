use syscall::io::{Io, Mmio};

#[repr(packed)]
pub struct CapabilityRegs {
    pub len: Mmio<u8>,
    _rsvd: Mmio<u8>,
    pub hci_ver: Mmio<u16>,
    pub hcs_params1: Mmio<u32>,
    pub hcs_params2: Mmio<u32>,
    pub hcs_params3: Mmio<u32>,
    pub hcc_params1: Mmio<u32>,
    pub db_offset: Mmio<u32>,
    pub rts_offset: Mmio<u32>,
    pub hcc_params2: Mmio<u32>,
}

pub const HCC_PARAMS1_AC64_BIT: u32 = 1 << HCC_PARAMS1_AC64_SHIFT;
pub const HCC_PARAMS1_AC64_SHIFT: u8 = 0;
pub const HCC_PARAMS1_MAXPSASIZE_MASK: u32 = 0xF000; // 15:12
pub const HCC_PARAMS1_MAXPSASIZE_SHIFT: u8 = 12;
pub const HCC_PARAMS1_XECP_MASK: u32 = 0xFFFF_0000;
pub const HCC_PARAMS1_XECP_SHIFT: u8 = 16;

pub const HCC_PARAMS2_LEC_BIT: u32 = 1 << 4;
pub const HCC_PARAMS2_CIC_BIT: u32 = 1 << 5;

pub const HCS_PARAMS1_MAX_PORTS_MASK: u32 = 0xFF00_0000;
pub const HCS_PARAMS1_MAX_PORTS_SHIFT: u8 = 24;
pub const HCS_PARAMS1_MAX_SLOTS_MASK: u32 = 0x0000_00FF;
pub const HCS_PARAMS1_MAX_SLOTS_SHIFT: u8 = 0;

pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_MASK: u32 = 0xF800_0000;
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_SHIFT: u8 = 27;
pub const HCS_PARAMS2_SPR_BIT: u32 = 1 << HCS_PARAMS2_SPR_SHIFT;
pub const HCS_PARAMS2_SPR_SHIFT: u8 = 26;
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_MASK: u32 = 0x03E0_0000;
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_SHIFT: u8 = 21;

impl CapabilityRegs {
    pub fn ac64(&self) -> bool {
        self.hcc_params1.readf(HCC_PARAMS1_AC64_BIT)
    }

    pub fn lec(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_LEC_BIT)
    }
    pub fn cic(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_CIC_BIT)
    }
    pub fn max_psa_size(&self) -> u8 {
        ((self.hcc_params1.read() & HCC_PARAMS1_MAXPSASIZE_MASK) >> HCC_PARAMS1_MAXPSASIZE_SHIFT)
            as u8
    }
    pub fn max_ports(&self) -> u8 {
        ((self.hcs_params1.read() & HCS_PARAMS1_MAX_PORTS_MASK) >> HCS_PARAMS1_MAX_PORTS_SHIFT)
            as u8
    }
    pub fn max_slots(&self) -> u8 {
        (self.hcs_params1.read() & HCS_PARAMS1_MAX_SLOTS_MASK) as u8
    }
    pub fn ext_caps_ptr_in_dwords(&self) -> u16 {
        ((self.hcc_params1.read() & HCC_PARAMS1_XECP_MASK) >> HCC_PARAMS1_XECP_SHIFT) as u16
    }
    pub fn max_scratchpad_bufs_lo(&self) -> u8 {
        ((self.hcs_params2.read() & HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_MASK) >> HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_SHIFT) as u8
    }
    pub fn spr(&self) -> bool {
        self.hcs_params2.readf(HCS_PARAMS2_SPR_BIT)
    }
    pub fn max_scratchpad_bufs_hi(&self) -> u8 {
        ((self.hcs_params2.read() & HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_MASK) >> HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_SHIFT) as u8
    }
    pub fn max_scratchpad_bufs(&self) -> u16 {
        u16::from(self.max_scratchpad_bufs_lo())
            | (u16::from(self.max_scratchpad_bufs_hi()) << 5)
    }
}
