use std::ptr;
use syscall::{Dma, Result};

/// A submission queue entry.
#[derive(Clone, Copy, Debug, Default)]
#[repr(packed)]
pub struct NvmeCmd {
    /// Opcode
    pub opcode: u8,
    /// Flags
    pub flags: u8,
    /// Command ID
    pub cid: u16,
    /// Namespace identifier
    pub nsid: u32,
    /// Reserved
    pub _rsvd: u64,
    /// Metadata pointer
    pub mptr: u64,
    /// Data pointer
    pub dptr: [u64; 2],
    /// Command dword 10
    pub cdw10: u32,
    /// Command dword 11
    pub cdw11: u32,
    /// Command dword 12
    pub cdw12: u32,
    /// Command dword 13
    pub cdw13: u32,
    /// Command dword 14
    pub cdw14: u32,
    /// Command dword 15
    pub cdw15: u32,
}

/// A completion queue entry.
#[derive(Clone, Copy, Debug)]
#[repr(packed)]
pub struct NvmeComp {
    pub command_specific: u32,
    pub _rsvd: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub status: u16,
}

/// Completion queue
pub struct NvmeCompQueue {
    pub data: Dma<[NvmeComp; 256]>,
    pub i: usize,
    pub phase: bool,
}

impl NvmeCompQueue {
    pub fn new() -> Result<Self> {
        Ok(Self {
            data: Dma::zeroed()?,
            i: 0,
            phase: true,
        })
    }

    /// Get a new completion queue entry, or return None if no entry is available yet.
    pub(crate) fn complete(&mut self) -> Option<(usize, NvmeComp)> {
        let entry = unsafe { ptr::read_volatile(self.data.as_ptr().add(self.i)) };
        // println!("{:?}", entry);
        if ((entry.status & 1) == 1) == self.phase {
            self.i = (self.i + 1) % self.data.len();
            if self.i == 0 {
                self.phase = !self.phase;
            }
            Some((self.i, entry))
        } else {
            None
        }
    }

    /// Get a new CQ entry, busy waiting until an entry appears.
    fn complete_spin(&mut self) -> (usize, NvmeComp) {
        loop {
            if let Some(some) = self.complete() {
                return some;
            } else {
                unsafe { std::arch::x86_64::_mm_pause() }
            }
        }
    }
}

/// Submission queue
pub struct NvmeCmdQueue {
    pub data: Dma<[NvmeCmd; 64]>,
    pub i: usize,
}

impl NvmeCmdQueue {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            data: Dma::zeroed()?,
            i: 0,
        })
    }

    /// Add a new submission command entry to the queue. Returns Some(tail) when a vacant entry was
    /// found, or None if the queue was full.
    pub(crate) fn submit(&mut self, entry: NvmeCmd) -> Option<usize> {
        // FIXME: Check for full conditions
        if true {
            self.data[self.i] = entry;
            self.i = (self.i + 1) % self.data.len();
            Some(self.i)
        } else {
            None
        }
    }
}
