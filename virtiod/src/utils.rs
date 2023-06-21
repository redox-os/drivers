use core::cell::UnsafeCell;
use core::marker::PhantomData;

#[derive(Debug)]
#[repr(C)]
pub struct VolatileCell<T> {
    value: UnsafeCell<T>,
}

impl<T: Copy> VolatileCell<T> {
    #[inline]
    pub fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    /// Returns a copy of the contained value.
    #[inline]
    pub fn get(&self) -> T {
        unsafe { core::ptr::read_volatile(self.value.get()) }
    }

    /// Sets the contained value.
    #[inline]
    pub fn set(&mut self, value: T) {
        unsafe { core::ptr::write_volatile(self.value.get(), value) }
    }
}

unsafe impl<T> Sync for VolatileCell<T> {}

#[repr(C)]
pub struct IncompleteArrayField<T>(PhantomData<T>, [T; 0]);

impl<T> IncompleteArrayField<T> {
    #[inline]
    pub const fn new() -> Self {
        IncompleteArrayField(PhantomData, [])
    }

    #[inline]
    pub unsafe fn as_slice(&self, len: usize) -> &[T] {
        core::slice::from_raw_parts(self.as_ptr(), len)
    }

    #[inline]
    pub unsafe fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        core::slice::from_raw_parts_mut(self.as_mut_ptr(), len)
    }

    #[inline]
    pub unsafe fn as_ptr(&self) -> *const T {
        self as *const _ as *const T
    }

    #[inline]
    pub unsafe fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut _ as *mut T
    }
}

pub const fn align(val: usize, align: usize) -> usize {
    (val + align) & !align
}

// From the syscall crate; the function is private.
//
// TODO(andypython): make it public
pub const fn round_up(x: usize) -> usize {
    (x + syscall::PAGE_SIZE - 1) / syscall::PAGE_SIZE * syscall::PAGE_SIZE
}
