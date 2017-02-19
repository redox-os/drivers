use syscall::io::{Io, Mmio};

#[repr(packed)]
pub struct NvmeCmd {
    /// Opcode
    opcode: u8,
    /// Flags
    flags: u8,
    /// Command ID
    cid: u16,
    /// Namespace identifier
    nsid: u32,
    /// Reserved
    _rsvd: u64,
    /// Metadata pointer
    mptr: u64,
    /// Data pointer
    dptr: [u64; 2],
    /// Command dword 10
    cdw10: u32,
    /// Command dword 11
    cdw11: u32,
    /// Command dword 12
    cdw12: u32,
    /// Command dword 13
    cdw13: u32,
    /// Command dword 14
    cdw14: u32,
    /// Command dword 15
    cdw15: u32,
}

impl NvmeCmd {
    pub fn read(cid: u16, lba: u64, count: u16, dst: u64) -> Self {
        NvmeCmd {
            opcode: 2,
            flags: 1 << 6,
            cid: cid,
            nsid: 0xFFFFFFFF,
            _rsvd: 0,
            mptr: 0,
            dptr: [dst, (count as u64) << 9],
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: count as u32,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn write(cid: u16, lba: u64, count: u16, src: u64) -> Self {
        NvmeCmd {
            opcode: 1,
            flags: 1 << 6,
            cid: cid,
            nsid: 0xFFFFFFFF,
            _rsvd: 0,
            mptr: 0,
            dptr: [src, (count as u64) << 9],
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: count as u32,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
}

#[repr(packed)]
pub struct NvmeRegs {
    /// Controller Capabilities
    cap: Mmio<u64>,
    /// Version
    vs: Mmio<u32>,
    /// Interrupt mask set
    intms: Mmio<u32>,
    /// Interrupt mask clear
    intmc: Mmio<u32>,
    /// Controller configuration
    cc: Mmio<u32>,
    /// Reserved
    _rsvd: Mmio<u32>,
    /// Controller status
    csts: Mmio<u32>,
    /// NVM subsystem reset
    nssr: Mmio<u32>,
    /// Admin queue attributes
    aqa: Mmio<u32>,
    /// Admin submission queue base address
    asq: Mmio<u64>,
    /// Admin completion queue base address
    acq: Mmio<u64>,
    /// Controller memory buffer location
    cmbloc: Mmio<u32>,
    /// Controller memory buffer size
    cmbsz: Mmio<u32>,
}

pub struct Nvme {
    regs: &'static mut NvmeRegs
}

impl Nvme {
    pub fn new(address: usize) -> Self {
        Nvme {
            regs: unsafe { &mut *(address as *mut NvmeRegs) }
        }
    }

    pub fn init(&mut self) {
        println!("  - CAPS: {:X}", self.regs.cap.read());
        println!("  - VS: {:X}", self.regs.vs.read());
    }
}
