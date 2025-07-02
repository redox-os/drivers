use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::slice;

use libredox::flag::{O_CLOEXEC, O_NONBLOCK, O_RDWR};
use orbclient::Event;
use syscall::ESTALE;

fn read_to_slice<T: Copy>(
    file: BorrowedFd,
    buf: &mut [T],
) -> Result<usize, libredox::error::Error> {
    unsafe {
        libredox::call::read(
            file.as_raw_fd() as usize,
            slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * size_of::<T>()),
        )
        .map(|count| count / size_of::<T>())
    }
}

unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    slice::from_raw_parts((p as *const T) as *const u8, size_of::<T>())
}

unsafe fn any_as_u8_slice_mut<T: Sized>(p: &mut T) -> &mut [u8] {
    slice::from_raw_parts_mut((p as *mut T) as *mut u8, size_of::<T>())
}

pub struct ConsumerHandle(File);

pub enum ConsumerHandleEvent<'a> {
    Events(&'a [Event]),
    Handoff,
}

impl ConsumerHandle {
    pub fn new_vt() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK as i32)
            .open(format!("/scheme/input/consumer"))?;
        Ok(Self(file))
    }

    pub fn event_handle(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    pub fn open_display(&self) -> io::Result<File> {
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

    pub fn open_display_v2(&self) -> io::Result<File> {
        let mut buffer = [0; 1024];
        let fd = self.0.as_raw_fd();
        let written = libredox::call::fpath(fd as usize, &mut buffer)?;

        assert!(written <= buffer.len());

        let mut display_path = PathBuf::from(
            std::str::from_utf8(&buffer[..written])
                .expect("init: display path UTF-8 check failed")
                .to_owned(),
        );
        display_path.set_file_name(format!(
            "v2/{}",
            display_path.file_name().unwrap().to_str().unwrap()
        ));
        let display_path = display_path.to_str().unwrap();

        let display_file =
            libredox::call::open(&display_path, (O_CLOEXEC | O_NONBLOCK | O_RDWR) as _, 0)
                .map(|socket| unsafe { File::from_raw_fd(socket as RawFd) })
                .unwrap_or_else(|err| {
                    panic!("failed to open display {}: {}", display_path, err);
                });

        Ok(display_file)
    }

    pub fn read_events<'a>(&self, events: &'a mut [Event]) -> io::Result<ConsumerHandleEvent<'a>> {
        match read_to_slice(self.0.as_fd(), events) {
            Ok(count) => Ok(ConsumerHandleEvent::Events(&events[..count])),
            Err(err) if err.errno() == ESTALE => Ok(ConsumerHandleEvent::Handoff),
            Err(err) => Err(err.into()),
        }
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct VtActivate {
    pub vt: usize,
}

pub struct DisplayHandle(File);

impl DisplayHandle {
    pub fn new<S: Into<String>>(device_name: S) -> io::Result<Self> {
        let path = format!("/scheme/input/handle/display/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    pub fn new_early<S: Into<String>>(device_name: S) -> io::Result<Self> {
        let path = format!("/scheme/input/handle_early/display/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    pub fn read_vt_event(&mut self) -> io::Result<Option<VtEvent>> {
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
    pub fn new() -> io::Result<Self> {
        let path = format!("/scheme/input/control");
        Ok(Self(File::open(path)?))
    }

    pub fn activate_vt(&mut self, vt: usize) -> io::Result<usize> {
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
    pub fn new() -> io::Result<Self> {
        File::open("/scheme/input/producer").map(ProducerHandle)
    }

    pub fn write_event(&mut self, event: orbclient::Event) -> io::Result<()> {
        self.0.write(&event)?;
        Ok(())
    }
}
