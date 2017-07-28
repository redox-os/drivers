use syscall::io::Mmio;

#[repr(packed)]
pub struct Interrupter {
    pub iman: Mmio<u32>,
    pub imod: Mmio<u32>,
    pub erstsz: Mmio<u32>,
    _rsvd: Mmio<u32>,
    pub erstba: Mmio<u64>,
    pub erdp: Mmio<u64>,
}

#[repr(packed)]
pub struct RuntimeRegs {
    pub mfindex: Mmio<u32>,
    _rsvd: [Mmio<u32>; 7],
    pub ints: [Interrupter; 1024],
}
