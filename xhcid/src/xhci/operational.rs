use syscall::io::{Io, Mmio};

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

pub const OP_CONFIG_CIE_BIT: u32 = 1 << 9;

impl OperationalRegs {
    pub fn cie(&self) -> bool {
        self.config.readf(OP_CONFIG_CIE_BIT)
    }
    pub fn set_cie(&mut self, value: bool) {
        self.config.writef(OP_CONFIG_CIE_BIT, value)
    }
}
