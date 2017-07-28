use syscall::io::Mmio;

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
