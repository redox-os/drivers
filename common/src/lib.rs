#![feature(int_roundings)]

use syscall::PAGE_SIZE;
use syscall::error::{Error, Result, EINVAL};
use syscall::flag::{MapFlags, O_CLOEXEC, O_RDONLY, O_RDWR, O_WRONLY};

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
    pub const RO: Self = Self { read: true, write: false };
    pub const WO: Self = Self { read: false, write: true };
    pub const RW: Self = Self { read: true, write: true };
}

pub unsafe fn physmap(base_phys: usize, len: usize, Prot { read, write }: Prot, ty: MemoryType) -> Result<*mut ()> {
    // TODO: arraystring?
    let path = format!("memory:physical@{}", match ty {
        MemoryType::Writeback => "wb",
        MemoryType::Uncacheable => "uc",
        MemoryType::WriteCombining => "wc",
        MemoryType::DeviceMemory => "dev",
    });
    let mode = match (read, write) {
        (true, true) => O_RDWR,
        (true, false) => O_RDONLY,
        (false, true) => O_WRONLY,
        (false, false) => return Err(Error::new(EINVAL)),
    };
    let mut prot = MapFlags::empty();
    prot.set(MapFlags::PROT_READ, read);
    prot.set(MapFlags::PROT_WRITE, write);

    let file = syscall::open(path, O_CLOEXEC | mode)?;
    let base = syscall::fmap(file, &syscall::Map {
        offset: base_phys,
        size: len.next_multiple_of(PAGE_SIZE),
        flags: MapFlags::MAP_SHARED | prot,
        address: 0,
    });
    let _ = syscall::close(file);

    Ok(base? as *mut ())
}
