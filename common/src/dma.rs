use std::mem::{self, MaybeUninit, size_of};
use std::ops::{Deref, DerefMut};
use std::ptr;

use syscall::PAGE_SIZE;

use syscall::Result;

use crate::MemoryType;

const DMA_MEMTY: MemoryType = {
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        // x86 ensures cache coherence with DMA memory
        MemoryType::Writeback
    } else if cfg!(target_arch = "aarch64") {
        // aarch64 currently must map DMA memory without caching to ensure coherence
        MemoryType::Uncacheable
    } else {
        panic!("invalid arch")
    }
};

fn alloc_and_map(len: usize) -> Result<(usize, *mut ())> {
    assert_eq!(len % PAGE_SIZE, 0);
    unsafe {
        let fd = syscall::open(format!("memory:zeroed@{DMA_MEMTY}?phys_contiguous"), syscall::O_CLOEXEC)?;
        let virt = syscall::fmap(fd, &syscall::Map {
            offset: 0, // ignored
            address: 0, // ignored
            size: len,
            flags: syscall::MapFlags::MAP_PRIVATE | syscall::MapFlags::PROT_READ | syscall::MapFlags::PROT_WRITE,
        });
        let _ = syscall::close(fd);
        let virt = virt?;
        let phys = syscall::virttophys(virt)?;
        /*for i in 1..len.div_ceil(PAGE_SIZE) {
            assert_eq!(syscall::virttophys(virt + i * PAGE_SIZE), Ok(phys + i * PAGE_SIZE), "NOT CONTIGUOUS");
        }*/
        Ok((phys, virt as *mut ()))
    }
}

pub struct Dma<T: ?Sized> {
    phys: usize,
    aligned_len: usize,
    virt: *mut T,
}

impl<T> Dma<T> {
    pub fn new(value: T) -> Result<Self> {
        unsafe {
            let mut zeroed = Self::zeroed()?;
            zeroed.as_mut_ptr().write(value);
            Ok(zeroed.assume_init())
        }
    }
    pub fn zeroed() -> Result<Dma<MaybeUninit<T>>> {
        let aligned_len = size_of::<T>().next_multiple_of(PAGE_SIZE);
        let (phys, virt) = alloc_and_map(aligned_len)?;
        Ok(Dma { phys, virt: virt.cast(), aligned_len })
    }
}

impl<T> Dma<MaybeUninit<T>> {
    pub unsafe fn assume_init(self) -> Dma<T> {
        let Dma { phys, aligned_len, virt } = self;
        mem::forget(self);

        Dma {
            phys,
            aligned_len,
            virt: virt.cast(),
        }
    }
}
impl<T: ?Sized> Dma<T> {
    pub fn physical(&self) -> usize {
        self.phys
    }
}

impl<T> Dma<[T]> {
    pub fn zeroed_slice(count: usize) -> Result<Dma<[MaybeUninit<T>]>> {
        let aligned_len = count.checked_mul(size_of::<T>()).unwrap().next_multiple_of(PAGE_SIZE);
        let (phys, virt) = alloc_and_map(aligned_len)?;

        Ok(Dma { phys, aligned_len, virt: ptr::slice_from_raw_parts_mut(virt.cast(), count) })
    }
}
impl<T> Dma<[MaybeUninit<T>]> {
    pub unsafe fn assume_init(self) -> Dma<[T]> {
        let &Dma { phys, aligned_len, virt } = &self;
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
            let _ = syscall::funmap(self.virt as *mut u8 as usize, self.aligned_len);
        }
    }
}
