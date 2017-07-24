use syscall::io::Mmio;

pub struct EventRingSte {
    pub address: Mmio<u64>,
    pub size: Mmio<u16>,
    _rsvd: Mmio<u16>,
    _rsvd2: Mmio<u32>,
}
