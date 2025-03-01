use std::fs::{File, OpenOptions};
use std::io::{Error, Read, Write};
use std::mem::size_of;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use libredox::flag::{O_CLOEXEC, O_NONBLOCK, O_RDWR};

unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    std::slice::from_raw_parts((p as *const T) as *const u8, size_of::<T>())
}

unsafe fn any_as_u8_slice_mut<T: Sized>(p: &mut T) -> &mut [u8] {
    std::slice::from_raw_parts_mut((p as *mut T) as *mut u8, size_of::<T>())
}

pub struct ConsumerHandle(File);

impl ConsumerHandle {
    pub fn for_vt(vt: usize) -> Result<Self, Error> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK as i32)
            .open(format!("/scheme/input/consumer/{vt}"))?;
        Ok(Self(file))
    }

    pub fn inner(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    pub fn open_display(&self) -> Result<File, Error> {
        let mut buffer = [0; 1024];
        let fd = self.0.as_raw_fd();
        let written = libredox::call::fpath(fd as usize, &mut buffer)?;

        assert!(written <= buffer.len());

        let display_path = std::str::from_utf8(&buffer[..written])
            .expect("init: display path UTF-8 check failed")
            .to_owned();

        let display_file =
            libredox::call::open(&display_path, (O_CLOEXEC | O_NONBLOCK | O_RDWR) as _, 0)
                .map(|socket| unsafe { File::from_raw_fd(socket as RawFd) })
                .unwrap_or_else(|err| {
                    panic!("failed to open display {}: {}", display_path, err);
                });

        Ok(display_file)
    }
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

    pub fn new_early<S: Into<String>>(device_name: S) -> Result<Self, Error> {
        let path = format!("/scheme/input/handle_early/display/{}", device_name.into());
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

pub struct ProducerHandle(File);

impl ProducerHandle {
    pub fn new() -> Result<Self, Error> {
        File::open("/scheme/input/producer").map(ProducerHandle)
    }

    pub fn write_event(&mut self, event: orbclient::Event) -> Result<(), Error> {
        self.0.write(&event)?;
        Ok(())
    }
}
