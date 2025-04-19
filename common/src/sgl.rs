use std::num::NonZeroUsize;

use libredox::call::MmapArgs;
use libredox::errno::EINVAL;
use libredox::error::{Error, Result};
use libredox::flag::{MAP_PRIVATE, PROT_READ, PROT_WRITE};
use syscall::{MAP_FIXED, PAGE_SIZE};

use crate::dma::phys_contiguous_fd;
use crate::VirtaddrTranslationHandle;

/// A Scatter-Gather List data structure
///
/// See: <https://en.wikipedia.org/wiki/Gather/scatter_(vector_addressing)>
#[derive(Debug)]
pub struct Sgl {
    /// A raw pointer to the SGL in virtual memory
    virt: *mut u8,
    /// The length of the allocated memory, guaranteed to be a multiple of [PAGE_SIZE].
    aligned_length: usize,
    /// The length of the allocated memory. This value is NOT guaranteed to be a multiple of [PAGE_SIZE]
    unaligned_length: NonZeroUsize,
    /// The vector of chunks tracked by this [Sgl] object. This is the sparsely-populated vector in the SGL algorithm.
    chunks: Vec<Chunk>,
}

/// A structure representing a chunk of memory in the sparsely-populated vector of the SGL
#[derive(Debug)]
pub struct Chunk {
    /// The offset of the chunk in the sparsely-populated vector.
    pub offset: usize,
    /// The physical address of the chunk
    pub phys: usize,
    /// A raw pointer to the chunk in virtual memory
    pub virt: *mut u8,
    /// The length of the chunk in bytes.
    pub length: usize,
}

impl Sgl {
    /// Constructor for the scatter/gather list.
    ///
    /// # Arguments
    ///
    /// 'unaligned_length: [usize]' - The length of the SGL, not necessarily aligned to the nearest
    /// page.
    pub fn new(unaligned_length: usize) -> Result<Self> {
        let unaligned_length = NonZeroUsize::new(unaligned_length).ok_or(Error::new(EINVAL))?;

        // TODO: Both PAGE_SIZE and MAX_ALLOC_SIZE should be dynamic.
        let aligned_length = unaligned_length.get().next_multiple_of(PAGE_SIZE);
        const MAX_ALLOC_SIZE: usize = 1 << 22;

        unsafe {
            let virt = libredox::call::mmap(MmapArgs {
                flags: MAP_PRIVATE,
                prot: PROT_READ | PROT_WRITE,
                length: aligned_length,

                offset: 0,
                fd: !0,
                addr: core::ptr::null_mut(),
            })?
            .cast::<u8>();

            let mut this = Self {
                virt,
                aligned_length,
                unaligned_length,
                chunks: Vec::new(),
            };

            // TODO: SglContext to avoid reopening these fds?
            let phys_contiguous_fd = phys_contiguous_fd()?;
            let virttophys_handle = VirtaddrTranslationHandle::new()?;

            let mut offset = 0;
            while offset < aligned_length {
                let preferred_chunk_length = (aligned_length - offset)
                    .min(MAX_ALLOC_SIZE)
                    .next_power_of_two();
                let chunk_length = if preferred_chunk_length > aligned_length - offset {
                    preferred_chunk_length / 2
                } else {
                    preferred_chunk_length
                };
                libredox::call::mmap(MmapArgs {
                    addr: virt.add(offset).cast(),
                    flags: MAP_PRIVATE | (MAP_FIXED.bits() as u32),
                    prot: PROT_READ | PROT_WRITE,
                    length: chunk_length,
                    fd: phys_contiguous_fd.raw(),

                    offset: 0,
                })?;
                let phys = virttophys_handle.translate(virt as usize + offset)?;
                this.chunks.push(Chunk {
                    offset,
                    phys,
                    length: (unaligned_length.get() - offset).min(chunk_length),
                    virt: virt.add(offset),
                });
                offset += chunk_length;
            }

            Ok(this)
        }
    }
    /// Returns an immutable reference to the vector of chunks
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Returns a raw pointer to the vector of chunks in virtual memory
    pub fn as_ptr(&self) -> *mut u8 {
        self.virt
    }
    /// Returns the length of the scatter-gather list.
    pub fn len(&self) -> usize {
        self.unaligned_length.get()
    }
}

impl Drop for Sgl {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.virt.cast(), self.aligned_length);
        }
    }
}
