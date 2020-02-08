use syscall::Mmio;

#[repr(packed)]
pub struct SupportedProtoCap {
    a: Mmio<u32>,
    b: Mmio<u32>,
    c: Mmio<u32>,
    d: Mmio<u32>,
    protocols: [Mmio<u32>],
}
