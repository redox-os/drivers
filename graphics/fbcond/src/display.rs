use libredox::flag;
use std::fs::OpenOptions;
use std::mem;
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::File,
    io,
    os::fd::RawFd,
    os::unix::io::{AsRawFd, FromRawFd},
    slice,
};
use syscall::{O_CLOEXEC, O_NONBLOCK, O_RDWR};

// Keep synced with vesad
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct SyncRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

fn display_fd_map(width: usize, height: usize, display_fd: usize) -> syscall::Result<*mut [u32]> {
    unsafe {
        let display_ptr = libredox::call::mmap(libredox::call::MmapArgs {
            fd: display_fd,
            offset: 0,
            length: (width * height * 4),
            prot: flag::PROT_READ | flag::PROT_WRITE,
            flags: flag::MAP_SHARED,
            addr: core::ptr::null_mut(),
        })?;
        let display_slice = slice::from_raw_parts_mut(display_ptr as *mut u32, width * height);
        Ok(display_slice)
    }
}

unsafe fn display_fd_unmap(image: *mut [u32]) {
    let _ = libredox::call::munmap(image as *mut (), image.len());
}

pub struct Display {
    pub input_handle: File,
    pub display_file: File,
    pub offscreen: *mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Display {
    pub fn open_vt(vt: usize) -> io::Result<Self> {
        let mut buffer = [0; 1024];

        let input_handle = OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK as i32)
            .open(format!("/scheme/input/consumer/{vt}"))?;
        let fd = input_handle.as_raw_fd();

        let written = libredox::call::fpath(fd as usize, &mut buffer)
            .expect("init: failed to get the path to the display device");

        assert!(written <= buffer.len());

        let display_path =
            std::str::from_utf8(&buffer[..written]).expect("init: display path UTF-8 check failed");

        let display_file =
            libredox::call::open(display_path, (O_CLOEXEC | O_NONBLOCK | O_RDWR) as _, 0)
                .map(|socket| unsafe { File::from_raw_fd(socket as RawFd) })
                .unwrap_or_else(|err| {
                    panic!("failed to open display {}: {}", display_path, err);
                });

        let mut buf: [u8; 4096] = [0; 4096];
        let count = libredox::call::fpath(display_file.as_raw_fd() as usize, &mut buf)
            .unwrap_or_else(|e| {
                panic!("Could not read display path with fpath(): {e}");
            });

        let url =
            String::from_utf8(Vec::from(&buf[..count])).expect("Could not create Utf8 Url String");
        let path = Self::url_parts(&url)?;
        let (width, height) = Self::parse_display_path(path);

        let offscreen_buffer = display_fd_map(width, height, display_file.as_raw_fd() as usize)
            .unwrap_or_else(|e| panic!("failed to map display '{display_path}: {e}"));
        Ok(Self {
            input_handle,
            display_file,
            offscreen: offscreen_buffer,
            width,
            height,
        })
    }

    fn url_parts(url: &str) -> io::Result<&str> {
        let mut url_parts = url.split(':');
        url_parts
            .next()
            .expect("Could not get scheme name from url");
        let path = url_parts.next().expect("Could not get path from url");
        Ok(path)
    }

    fn parse_display_path(path: &str) -> (usize, usize) {
        let mut path_parts = path.split('/').skip(1);
        let width = path_parts
            .next()
            .unwrap_or("")
            .parse::<usize>()
            .unwrap_or(0);
        let height = path_parts
            .next()
            .unwrap_or("")
            .parse::<usize>()
            .unwrap_or(0);

        (width, height)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        match display_fd_map(width, height, self.display_file.as_raw_fd() as usize) {
            Ok(ok) => {
                unsafe {
                    display_fd_unmap(self.offscreen);
                }
                self.offscreen = ok;
                self.width = width;
                self.height = height;
            }
            Err(err) => {
                eprintln!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    pub fn sync_rect(&mut self, sync_rect: SyncRect) -> syscall::Result<()> {
        unsafe {
            libredox::call::write(
                self.display_file.as_raw_fd().as_raw_fd() as usize,
                slice::from_raw_parts(
                    &sync_rect as *const SyncRect as *const u8,
                    mem::size_of::<SyncRect>(),
                ),
            )?;
            Ok(())
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            display_fd_unmap(self.offscreen);
        }
    }
}
