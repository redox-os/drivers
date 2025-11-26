//! This crate provides various abstractions for use by all drivers in the Redox drivers repo.
//!
//! This includes direct memory access via [dma], and Scatter-Gather List support via [sgl].  It also
//! provides various memory management structures for use with drivers, and some logging support.
#![warn(missing_docs)]

use libredox::call::MmapArgs;
use libredox::flag::{self, O_CLOEXEC, O_RDONLY, O_RDWR, O_WRONLY};
use libredox::{errno::EINVAL, error::*, Fd};
use syscall::{ProcSchemeVerb, PAGE_SIZE};

/// The Direct Memory Access (DMA) API for drivers
pub mod dma;
/// MMIO utilities
pub mod io;
mod logger;
/// The Scatter Gather List (SGL) API for drivers.
pub mod sgl;
/// Low latency timeout for driver loops
pub mod timeout;

pub use logger::{output_level, file_level, setup_logging};

/// Specifies the write behavior for a specific region of memory
///
/// These types indicate to the driver how writes to a specific memory region are handled by the
/// system. This usually refers to the caching behavior that the processor or I/O device responsible
/// for that memory implements.
///
/// aarch64 and x86 have very different cache-coherency rules, so this API as written is likely
/// not sufficient to describe the memory caching behavior in a cross-platform manner. As such,
/// consider this API unstable.
#[derive(Clone, Copy, Debug)]
pub enum MemoryType {
    /// A region of memory that implements Write-back caching.
    ///
    /// In write-back caching, the processor will first store data in its local cache, and then
    /// flush it to the actual storage location at regular intervals, or as applications access
    /// the data.
    Writeback,
    /// A region of memory that does not implement caching. Writes to these regions are immediate.
    Uncacheable,
    /// A region of memory that implements write combining.
    ///
    /// Write combining memory regions store all writes in a temporary buffer called a Write
    /// Combine Buffer. Multiple writes to the location are stored in a single buffer, and then
    /// released to the memory location in an unspecified order. Write-Combine memory does not
    /// guarantee that the order at which you write to it is the order at which those writes are
    /// committed to memory.
    WriteCombining,
    /// Memory stored in an intermediate Write Combine Buffer and released later
    /// Memory-Mapped I/O. This is an aarch64-specific term.
    DeviceMemory,
}
impl Default for MemoryType {
    fn default() -> Self {
        Self::Writeback
    }
}

/// Represents the protection level of an area of memory.
///
/// This structure shouldn't be used directly -- instead, use the [Prot::RO] (Read-Only),
/// [Prot::WO] (Write-Only) and [Prot::RW] (Read-Write) constants to specify the memory's protection
/// level.
#[derive(Clone, Copy, Debug)]
pub struct Prot {
    /// The memory is readable
    pub read: bool,
    /// The memory is writeable
    pub write: bool,
}

/// Implements the memory protection level constants
impl Prot {
    /// A constant representing Read-Only memory.
    pub const RO: Self = Self {
        read: true,
        write: false,
    };

    /// A constant representing Write-Only memory
    pub const WO: Self = Self {
        read: false,
        write: true,
    };

    /// A constant representing Read-Write memory
    pub const RW: Self = Self {
        read: true,
        write: true,
    };
}

// TODO: Safe, as the kernel ensures it doesn't conflict with any other memory described in the
// memory map for regular RAM.
/// Maps physical memory to virtual memory
///
/// # Arguments
///
/// * 'base_phys: [usize]' - The base address of the physical memory to map.
/// * 'len: [usize]'       - The length of the physical memory to map (Should be a multiple of [PAGE_SIZE]
/// * '_: [Prot]'          - The memory protection level of the mapping.
/// * 'type: [MemoryType]' - The caching behavior specification of the memory.
///
/// # Returns
///
/// A '[Result]<*mut ()>' which is:
/// - '[Ok]'  containing a raw pointer to the mapped memory.
/// - '[Err]' which contains an error on failure.
///
/// # Errors
///
/// This function will return an error if:
/// - An invalid value is provided to 'read' or 'write'
/// - The system could not open a file descriptor to the memory scheme for the specified [MemoryType].
/// - The system failed to map the physical address to a virtual address. See [libredox::call::mmap]
///
///
/// # Notes
/// - This function is unsafe, and upon using it you will be responsible for freeing the memory with
///   [libredox::call::munmap]. If you want a safe accessor, use [PhysBorrowed] instead.
/// - The MemoryType specified is used to tell the function which memory scheme to access. (i.e
///   /scheme/memory/physical@wb, /scheme/memory/physical@uc, etc).
pub unsafe fn physmap(
    base_phys: usize,
    len: usize,
    Prot { read, write }: Prot,
    ty: MemoryType,
) -> Result<*mut ()> {
    // TODO: arraystring?

    //Return an error rather than potentially crash the kernel.
    if base_phys == 0 {
        return Err(Error::new(EINVAL));
    }

    let path = format!(
        "/scheme/memory/physical@{}",
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

/// A safe virtual mapping to physical memory that unmaps the memory when the structure goes out
/// of scope.
///
/// This function provides a safe binding to [physmap]. It implements Drop to free the mapped memory
/// when the structure goes out of scope.
pub struct PhysBorrowed {
    mem: *mut (),
    len: usize,
}
impl PhysBorrowed {
    /// Constructs a PhysBorrowed instance.
    ///
    /// # Arguments
    /// See [physmap] for a description of the parameters.
    ///
    /// # Returns
    /// A '[Result]' which contains the following:
    /// - A '[PhysBorrowed]' which represents the newly mapped region.
    /// - An 'Err' if a memory mapping error occurs.
    ///
    /// # Errors
    /// See [physmap] for a description of the error cases.
    pub fn map(base_phys: usize, len: usize, prot: Prot, ty: MemoryType) -> Result<Self> {
        let mem = unsafe { physmap(base_phys, len, prot, ty)? };
        Ok(Self {
            mem,
            len: len.next_multiple_of(PAGE_SIZE),
        })
    }

    /// Gets a raw pointer to the borrowed region.
    ///
    /// # Returns
    /// - self.mem - A pointer to the mapped region in virtual memory.
    ///
    /// # Notes
    /// - The pointer may live beyond the lifetime of [PhysBorrowed], so dereferences to the pointer
    ///   must be treated as unsafe.
    ///
    pub fn as_ptr(&self) -> *mut () {
        self.mem
    }

    /// Gets the length of the mapped region.
    ///
    /// # Returns
    /// - self.len - The length of the mapped region. It should be a multiple of [PAGE_SIZE]
    pub fn mapped_len(&self) -> usize {
        self.len
    }
}

impl Drop for PhysBorrowed {
    /// Frees the mapped memory region.
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.mem, self.len);
        }
    }
}

// TODO: temporary wrapper in redox_syscall?
unsafe fn sys_call(fd: usize, buf: &mut [u8], metadata: &[u64]) -> Result<usize> {
    Ok(syscall::syscall5(
        syscall::SYS_CALL,
        fd,
        buf.as_mut_ptr() as usize,
        buf.len(),
        metadata.len(),
        metadata.as_ptr() as usize,
    )?)
}

/// Instructs the kernel to enable I/O ports for this (usermode) process (x86-specific).
///
/// On Redox, x86 privilege ring 3 represents userspace. Most Redox drivers run in userspace to
/// prevent system instability caused by a faulty driver. Processes with (bitmap-enabled) IO port
/// rights can use the IN/OUT instructions. This is not the same as IOPL 3; the CLI instruction is
/// still not allowed.
pub fn acquire_port_io_rights() -> Result<()> {
    extern "C" {
        fn redox_cur_thrfd_v0() -> usize;
    }
    let kernel_fd = syscall::dup(unsafe { redox_cur_thrfd_v0() }, b"open_via_dup")?;
    let res = unsafe { sys_call(kernel_fd, &mut [], &[ProcSchemeVerb::Iopl as u64]) };
    let _ = syscall::close(kernel_fd);
    res?;
    Ok(())
}

/// Kernel handle for translating virtual addresses in the current address space, to their
/// underlying physical addresses.
///
/// It is currently unspecified whether this handle is specific to the address space at the time it
/// was created, or whether all calls reference the currently active address space.
pub struct VirtaddrTranslationHandle {
    fd: Fd,
}

impl VirtaddrTranslationHandle {
    /// Create a new handle, requires uid=0 but this may change.
    pub fn new() -> Result<Self> {
        Ok(Self {
            fd: Fd::open("/scheme/memory/translation", O_CLOEXEC, 0)?,
        })
    }
    /// Translate physical => virtual.
    pub fn translate(&self, physical: usize) -> Result<usize> {
        let mut buf = physical.to_ne_bytes();
        unsafe {
            sys_call(self.fd.raw(), &mut buf, &[])?;
        }
        Ok(usize::from_ne_bytes(buf))
    }
}
