#![feature(iter_next_chunk)]

use std::fs::File;
use std::io::{Error, Read, Write};
use std::mem::size_of;
use std::os::fd::{AsFd, BorrowedFd};

unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    std::slice::from_raw_parts((p as *const T) as *const u8, size_of::<T>())
}

unsafe fn any_as_u8_slice_mut<T: Sized>(p: &mut T) -> &mut [u8] {
    std::slice::from_raw_parts_mut((p as *mut T) as *mut u8, size_of::<T>())
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct VtActivate {
    pub vt: usize,
}

pub struct DisplayHandle(File);

impl DisplayHandle {
    pub fn new<S: Into<String>>(device_name: S) -> Result<Self, Error> {
        let path = format!("/scheme/input/handle/display/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    // The return value is the display identifier. It will be used to uniquely
    // identify the display on activation events.
    pub fn register_vt(&mut self) -> Result<usize, Error> {
        self.0.read(&mut [])
    }

    pub fn read_vt_event(&mut self) -> Result<Option<VtEvent>, Error> {
        let mut event = VtEvent {
            kind: VtEventKind::Resize,
            vt: usize::MAX,
            width: u32::MAX,
            height: u32::MAX,
            stride: u32::MAX,
        };

        let nread = self.0.read(unsafe { any_as_u8_slice_mut(&mut event) })?;

        if nread == 0 {
            Ok(None)
        } else {
            assert_eq!(nread, size_of::<VtEvent>());
            Ok(Some(event))
        }
    }

    pub fn inner(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

pub struct ControlHandle(File);

impl ControlHandle {
    pub fn new() -> Result<Self, Error> {
        let path = format!("/scheme/input/control");
        Ok(Self(File::open(path)?))
    }

    pub fn activate_vt(&mut self, vt: usize) -> Result<usize, Error> {
        let cmd = VtActivate { vt };
        self.0.write(unsafe { any_as_u8_slice(&cmd) })
    }
}

#[derive(Debug)]
#[repr(usize)]
pub enum VtEventKind {
    Activate,
    Deactivate,
    Resize,
}

#[derive(Debug)]
#[repr(C)]
pub struct VtEvent {
    pub kind: VtEventKind,
    pub vt: usize,

    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

#[repr(packed)]
pub struct Damage {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
