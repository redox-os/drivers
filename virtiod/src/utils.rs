use core::cell::UnsafeCell;
use core::marker::PhantomData;

use static_assertions::const_assert_eq;

#[derive(Debug)]
#[repr(transparent)]
pub struct VolatileCell<T> {
    value: UnsafeCell<T>,
}

impl<T: Copy> VolatileCell<T> {
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

#[repr(C)]
pub struct IncompleteArrayField<T>(PhantomData<T>, [T; 0]);

impl<T> IncompleteArrayField<T> {
    #[inline]
    pub const fn new() -> Self {
        IncompleteArrayField(PhantomData, [])
    }
}

pub const fn align(val: usize, align: usize) -> usize {
    (val + align) & !align
}
