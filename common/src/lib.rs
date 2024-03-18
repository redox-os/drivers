#![feature(int_roundings)]

use libredox::call::MmapArgs;
use libredox::{Fd, error::*, errno::EINVAL};
use libredox::flag::{self, O_CLOEXEC, O_RDONLY, O_RDWR, O_WRONLY};
use syscall::PAGE_SIZE;

pub mod dma;

#[derive(Clone, Copy, Debug)]
pub enum MemoryType {
    Writeback,
    Uncacheable,
    WriteCombining,
    DeviceMemory,
}
impl Default for MemoryType {
    fn default() -> Self {
        Self::Writeback
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Prot {
    pub read: bool,
    pub write: bool,
}
impl Prot {
    pub const RO: Self = Self {
        read: true,
        write: false,
    };
    pub const WO: Self = Self {
        read: false,
        write: true,
    };
    pub const RW: Self = Self {
        read: true,
        write: true,
    };
}

// TODO: Safe, as the kernel ensures it doesn't conflict with any other memory described in the
// memory map for regular RAM.
pub unsafe fn physmap(
    base_phys: usize,
    len: usize,
    Prot { read, write }: Prot,
    ty: MemoryType,
) -> Result<*mut ()> {
    // TODO: arraystring?
    let path = format!(
        "memory:physical@{}",
        match ty {
            MemoryType::Writeback => "wb",
            MemoryType::Uncacheable => "uc",
            MemoryType::WriteCombining => "wc",
            MemoryType::DeviceMemory => "dev",
        }
    );
    let mode = match (read, write) {
        (true, true) => O_RDWR,
        (true, false) => O_RDONLY,
        (false, true) => O_WRONLY,
        (false, false) => return Err(Error::new(EINVAL)),
    };
    let mut prot = 0;
    if read {
        prot |= flag::PROT_READ;
    }
    if write {
        prot |= flag::PROT_WRITE;
    }

    let fd = Fd::open(&path, O_CLOEXEC | mode, 0)?;
    Ok(libredox::call::mmap(MmapArgs {
        fd: fd.raw(),
        offset: base_phys as u64,
        length: len.next_multiple_of(PAGE_SIZE),
        flags: flag::MAP_SHARED,
        prot,
        addr: core::ptr::null_mut(),
    })? as *mut ())
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Writeback => "wb",
                Self::Uncacheable => "uc",
                Self::WriteCombining => "wc",
                Self::DeviceMemory => "dev",
            }
        )
    }
}

pub struct PhysBorrowed {
    mem: *mut (),
    len: usize,
}
impl PhysBorrowed {
    pub fn map(base_phys: usize, len: usize, prot: Prot, ty: MemoryType) -> Result<Self> {
        let mem = unsafe { physmap(base_phys, len, prot, ty)? };
        Ok(Self {
            mem,
            len: len.next_multiple_of(PAGE_SIZE),
        })
    }
    pub fn as_ptr(&self) -> *mut () {
        self.mem
    }
    pub fn mapped_len(&self) -> usize {
        self.len
    }
}
impl Drop for PhysBorrowed {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.mem, self.len);
        }
    }
}

pub fn acquire_port_io_rights() -> Result<()> {
    unsafe {
        syscall::iopl(3)?;
    }
    Ok(())
}
