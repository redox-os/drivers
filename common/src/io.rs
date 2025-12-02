use core::{
    cmp::PartialEq,
    ops::{BitAnd, BitOr, Not},
};

mod mmio;
mod mmio_ptr;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod pio;

pub use mmio::*;
pub use mmio_ptr::*;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub use pio::*;

/// IO abstraction
pub trait Io {
    /// Value type for IO, usually some unsigned number
    type Value: Copy
        + PartialEq
        + BitAnd<Output = Self::Value>
        + BitOr<Output = Self::Value>
        + Not<Output = Self::Value>;

    /// Read the underlying valu2e
    fn read(&self) -> Self::Value;
    /// Write the underlying value
    fn write(&mut self, value: Self::Value);

    /// Check whether the underlying value contains bit flags
    #[inline(always)]
    fn readf(&self, flags: Self::Value) -> bool {
        (self.read() & flags) as Self::Value == flags
    }

    /// Enable or disable specific bit flags
    #[inline(always)]
    fn writef(&mut self, flags: Self::Value, value: bool) {
        let tmp: Self::Value = match value {
            true => self.read() | flags,
            false => self.read() & !flags,
        };
        self.write(tmp);
    }
}

/// Read-only IO
#[repr(transparent)]
pub struct ReadOnly<I> {
    inner: I,
}

impl<I: Io> ReadOnly<I> {
    /// Wraps IO
    pub const fn new(inner: I) -> ReadOnly<I> {
        ReadOnly { inner }
    }
}

impl<I: Io> ReadOnly<I> {
    /// Calls [Io::read]
    #[inline(always)]
    pub fn read(&self) -> I::Value {
        self.inner.read()
    }

    /// Calls [Io::readf]
    #[inline(always)]
    pub fn readf(&self, flags: I::Value) -> bool {
        self.inner.readf(flags)
    }
}

#[repr(transparent)]
/// Write-only IO
pub struct WriteOnly<I> {
    inner: I,
}

impl<I: Io> WriteOnly<I> {
    /// Wraps IO
    pub const fn new(inner: I) -> WriteOnly<I> {
        WriteOnly { inner }
    }
}

impl<I: Io> WriteOnly<I> {
    /// Calls [Io::write]
    #[inline(always)]
    pub fn write(&mut self, value: I::Value) {
        self.inner.write(value)
    }

    // writef requires read which is not valid when write-only
}
