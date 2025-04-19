use std::mem::{self, size_of, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::LazyLock;

use libredox::call::MmapArgs;
use libredox::{error::Result, flag, Fd};
use syscall::PAGE_SIZE;

use crate::{MemoryType, VirtaddrTranslationHandle};

/// Defines the platform-specific memory type for DMA operations
///
/// - On x86 systems, DMA uses Write-back memory ([MemoryType::Writeback])
/// - On aarch64 systems, DMA uses uncacheable memory ([MemoryType::Uncacheable])
const DMA_MEMTY: MemoryType = {
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        // x86 ensures cache coherence with DMA memory
        MemoryType::Writeback
    } else if cfg!(target_arch = "aarch64") {
        // aarch64 currently must map DMA memory without caching to ensure coherence
        MemoryType::Uncacheable
    } else if cfg!(target_arch = "riscv64") {
        // FIXME check this out more
        MemoryType::Uncacheable
    } else {
        panic!("invalid arch")
    }
};

/// Returns a file descriptor for zeroized physically-contiguous DMA memory.
///
/// # Returns
///
/// A [Result] containing:
/// - '[Ok]' - A [Fd] (file descriptor) to zeroized, physically continuous DMA usable memory
/// - '[Err]' - The error returned by the provider of the /scheme/memory/zeroed scheme.
///
/// # Errors
///
/// This function can return an error in the following case:
///
/// - The request for the physical memory fails.
pub(crate) fn phys_contiguous_fd() -> Result<Fd> {
    Fd::open(
        &format!("/scheme/memory/zeroed@{DMA_MEMTY}?phys_contiguous"),
        flag::O_CLOEXEC,
        0,
    )
}

/// Allocates a chunk of physical memory for DMA, and then maps it to virtual memory.
///
/// # Arguments
/// 'length: [usize]' - The length of the memory region. Must be a multiple of [PAGE_SIZE]
///
/// # Returns
///
/// This function returns a [Result] containing the following:
/// - A  '[Ok]([usize], *[mut] ())' containing a Tuple of the physical address of the region, and a raw pointer to that region in virtual memory.
/// - An '[Err]' - containing the error for the operation.
///
/// # Errors
///
/// This function asserts if:
/// - length is not a multiple of [PAGE_SIZE]
///
/// This function returns an error if:
/// - A file descriptor to physically contiguous memory of type [DMA_MEMTY] could not be acquired
/// - A virtual mapping for the physically contiguous memory could not be created
/// - The virtual address returned by the memory manager was invalid.
fn alloc_and_map(length: usize, handle: &VirtaddrTranslationHandle) -> Result<(usize, *mut ())> {
    assert_eq!(length % PAGE_SIZE, 0);
    unsafe {
        let fd = phys_contiguous_fd()?;
        let virt = libredox::call::mmap(MmapArgs {
            fd: fd.raw(),
            offset: 0,                   // ignored
            addr: core::ptr::null_mut(), // ignored
            length,
            flags: flag::MAP_PRIVATE,
            prot: flag::PROT_READ | flag::PROT_WRITE,
        })?;
        let phys = handle.translate(virt as usize)?;
        for i in 1..length.div_ceil(PAGE_SIZE) {
            debug_assert_eq!(
                handle.translate(virt as usize + i * PAGE_SIZE),
                Ok(phys + i * PAGE_SIZE),
                "NOT CONTIGUOUS"
            );
        }
        Ok((phys, virt as *mut ()))
    }
}

/// A safe accessor for DMA memory.
pub struct Dma<T: ?Sized> {
    /// The physical address of the memory
    phys: usize,
    /// The page-aligned length of the memory. Will be a multiple of [PAGE_SIZE]
    aligned_len: usize,
    /// The pointer to the Dma memory in the virtual address space.
    virt: *mut T,
}

impl<T> Dma<T> {
    /// [Dma] constructor that allocates and initializes a region of DMA memory with the page-aligned
    /// size and initial value of some T
    ///
    /// # Arguments
    /// 'value: T' - The initial value to write to the allocated region
    ///
    /// # Returns
    ///
    /// This function returns a [Result] containing the following:
    ///
    /// - A '[Ok] (`[Dma]<T>`)' containing the initialized region
    /// - An '[Err]' containing an error.
    pub fn new(value: T) -> Result<Self> {
        unsafe {
            let mut zeroed = Self::zeroed()?;
            zeroed.as_mut_ptr().write(value);
            Ok(zeroed.assume_init())
        }
    }

    /// [Dma] constructor that allocates and zeroizes a memory region of the page-aligned size of T
    ///
    /// # Returns
    ///
    /// This function returns a [Result] containing the following:
    ///
    /// - A '[Ok] (`[Dma]<[MaybeUninit]<T>>`)' containing the allocated and zeroized memory
    /// - An '[Err]' containing an error.
    pub fn zeroed() -> Result<Dma<MaybeUninit<T>>> {
        let aligned_len = size_of::<T>().next_multiple_of(PAGE_SIZE);
        let (phys, virt) = alloc_and_map(aligned_len, &*VIRTTOPHYS_HANDLE)?;
        Ok(Dma {
            phys,
            virt: virt.cast(),
            aligned_len,
        })
    }
}

impl<T> Dma<MaybeUninit<T>> {
    /// Assumes that possibly uninitialized DMA memory has been initialized, and returns a new
    /// instance of an object of type `[Dma]<T>`.
    ///
    /// # Returns
    /// - `[Dma]<T>` - The original structure without the [MaybeUninit] wrapper around its contents.
    ///
    /// # Notes
    /// - This is unsafe because it assumes that the memory stored within the `[Dma]<T>` is a valid
    ///   instance of T. If it isn't (for example -- if it was initialized with [Dma::zeroed]),
    ///   then the underlying memory may not contain the expected T structure.
    pub unsafe fn assume_init(self) -> Dma<T> {
        let Dma {
            phys,
            aligned_len,
            virt,
        } = self;
        mem::forget(self);

        Dma {
            phys,
            aligned_len,
            virt: virt.cast(),
        }
    }
}
impl<T: ?Sized> Dma<T> {
    /// Returns the physical address of the physical memory that this [Dma] structure references.
    ///
    /// # Returns
    /// [usize] - The physical address of the memory.
    pub fn physical(&self) -> usize {
        self.phys
    }
}
// TODO: there should exist a "context" struct that drivers create at start, which would be passed
// to the respective functions
static VIRTTOPHYS_HANDLE: LazyLock<VirtaddrTranslationHandle> = LazyLock::new(|| {
    VirtaddrTranslationHandle::new().expect("failed to acquire virttophys translation handle")
});

impl<T> Dma<[T]> {
    /// Returns a [Dma] object containing a zeroized slice of T with a given count.
    ///
    /// # Arguments
    ///
    /// - 'count: [usize]' - The number of elements of type T in the allocated slice.
    pub fn zeroed_slice(count: usize) -> Result<Dma<[MaybeUninit<T>]>> {
        let aligned_len = count
            .checked_mul(size_of::<T>())
            .unwrap()
            .next_multiple_of(PAGE_SIZE);
        let (phys, virt) = alloc_and_map(aligned_len, &*VIRTTOPHYS_HANDLE)?;

        Ok(Dma {
            phys,
            aligned_len,
            virt: ptr::slice_from_raw_parts_mut(virt.cast(), count),
        })
    }

    /// Casts the slice from type T to type U.
    ///
    /// # Returns
    /// '`[DMA]<U>`' - A cast handle to the Dma memory.
    pub unsafe fn cast_slice<U>(self) -> Dma<[U]> {
        let Dma {
            phys,
            virt,
            aligned_len,
        } = self;
        core::mem::forget(self);

        Dma {
            phys,
            virt: virt as *mut [U],
            aligned_len,
        }
    }
}
impl<T> Dma<[MaybeUninit<T>]> {
    /// See [`Dma<MaybeUninit<T>>::assume_init`]
    pub unsafe fn assume_init(self) -> Dma<[T]> {
        let &Dma {
            phys,
            aligned_len,
            virt,
        } = &self;
        mem::forget(self);

        Dma {
            phys,
            aligned_len,
            virt: virt as *mut [T],
        }
    }
}

impl<T: ?Sized> Deref for Dma<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.virt }
    }
}

impl<T: ?Sized> DerefMut for Dma<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.virt }
    }
}

impl<T: ?Sized> Drop for Dma<T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.virt);
            let _ = libredox::call::munmap(self.virt as *mut (), self.aligned_len);
        }
    }
}
