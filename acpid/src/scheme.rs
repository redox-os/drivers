use syscall::error::{EBADF, ENOENT};
use syscall::error::{Error, Result};
use syscall::scheme::Scheme;

use crate::acpi::AcpiContext;

pub struct AcpiScheme<'acpi> {
    ctx: &'acpi AcpiContext,
}

impl<'acpi> AcpiScheme<'acpi> {
    pub fn new(ctx: &'acpi AcpiContext) -> Self {
        Self {
            ctx,
        }
    }
}

const ALLOWED_TABLE_SIGNATURES: [[u8; 4]; 1] = [*b"MCFG"];

impl Scheme for AcpiScheme<'_> {
    fn open(&self, path: &str, flags: usize, uid: u32, gid: u32) -> Result<usize> {
        Err(Error::new(ENOENT))
    }
    fn seek(&self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        Err(Error::new(EBADF))
    }
    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        Err(Error::new(EBADF))
    }
    fn write(&self, id: usize, buf: &[u8]) -> Result<usize> {
        Err(Error::new(EBADF))
    }
    fn close(&self, id: usize) -> Result<usize> {
        Err(Error::new(EBADF))
    }
}
