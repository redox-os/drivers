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
    pub hcc_params2: Mmio<u32>
}

pub const HCC_PARAMS1_MAXPSASIZE_MASK: u32 = 0xF000; // 15:12
pub const HCC_PARAMS1_MAXPSASIZE_SHIFT: u8 = 12;

pub const HCC_PARAMS2_LEC_BIT: u32 = 1 << 4;
pub const HCC_PARAMS2_CIC_BIT: u32 = 1 << 5;

impl CapabilityRegs {
    pub fn lec(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_LEC_BIT)
    }
    pub fn cic(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_CIC_BIT)
    }
    pub fn max_psa_size(&self) -> u8 {
        ((self.hcc_params1.read() & HCC_PARAMS1_MAXPSASIZE_MASK) >> HCC_PARAMS1_MAXPSASIZE_SHIFT) as u8
    }
}
