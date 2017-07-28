use syscall::io::Mmio;

#[repr(packed)]
pub struct OperationalRegs {
    pub usb_cmd: Mmio<u32>,
    pub usb_sts: Mmio<u32>,
    pub page_size: Mmio<u32>,
    _rsvd: [Mmio<u32>; 2],
    pub dn_ctrl: Mmio<u32>,
    pub crcr: Mmio<u64>,
    _rsvd2: [Mmio<u32>; 4],
    pub dcbaap: Mmio<u64>,
    pub config: Mmio<u32>,
}
