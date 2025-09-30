use common::io::Mmio;

#[repr(C, packed)]
pub struct Interrupter {
    pub iman: Mmio<u32>,
    pub imod: Mmio<u32>,
    pub erstsz: Mmio<u32>,
    _rsvd: Mmio<u32>,
    pub erstba_low: Mmio<u32>,
    pub erstba_high: Mmio<u32>,
    pub erdp_low: Mmio<u32>,
    pub erdp_high: Mmio<u32>,
}

#[repr(C, packed)]
pub struct RuntimeRegs {
    pub mfindex: Mmio<u32>,
    _rsvd: [Mmio<u32>; 7],
    pub ints: [Interrupter; 1024],
}
