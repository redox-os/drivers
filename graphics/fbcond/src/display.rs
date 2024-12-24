use inputd::{ConsumerHandle, Damage};
use libredox::flag;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::{io, mem, ptr, slice};

fn display_fd_map(
    width: usize,
    height: usize,
    display_file: &mut File,
) -> syscall::Result<*mut [u32]> {
    unsafe {
        let display_ptr = libredox::call::mmap(libredox::call::MmapArgs {
            fd: display_file.as_raw_fd() as usize,
            offset: 0,
            length: (width * height * 4),
            prot: flag::PROT_READ | flag::PROT_WRITE,
            flags: flag::MAP_SHARED,
            addr: core::ptr::null_mut(),
        })?;
        Ok(ptr::slice_from_raw_parts_mut(
            display_ptr as *mut u32,
            width * height,
        ))
    }
}

unsafe fn display_fd_unmap(image: *mut [u32]) {
    let _ = libredox::call::munmap(image as *mut (), image.len());
}

pub struct Display {
    pub input_handle: ConsumerHandle,
    pub display_file: File,
    pub offscreen: *mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Display {
    pub fn open_vt(vt: usize) -> io::Result<Self> {
        let input_handle = ConsumerHandle::for_vt(vt)?;

        let (mut display_file, width, height) = Self::open_display(&input_handle)?;

        let offscreen = display_fd_map(width, height, &mut display_file)
            .unwrap_or_else(|e| panic!("failed to map display for VT #{vt}: {e}"));

        Ok(Self {
            input_handle,
            display_file,
            offscreen,
            width,
            height,
        })
    }

    /// Re-open the display after a handoff.
    pub fn reopen_for_handoff(&mut self) {
        eprintln!("fbcond: Performing handoff");

        let (mut new_display_file, width, height) = Self::open_display(&self.input_handle).unwrap();

        eprintln!("fbcond: Opened new display");

        match display_fd_map(width, height, &mut new_display_file) {
            Ok(offscreen) => {
                self.offscreen = offscreen;
                self.display_file = new_display_file;

                eprintln!("fbcond: Mapped new display");
            }
            Err(err) => {
                eprintln!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    fn open_display(input_handle: &ConsumerHandle) -> io::Result<(File, usize, usize)> {
        let display_file = input_handle.open_display()?;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = libredox::call::fpath(display_file.as_raw_fd() as usize, &mut buf)
            .unwrap_or_else(|e| {
                panic!("Could not read display path with fpath(): {e}");
            });

        let url =
            String::from_utf8(Vec::from(&buf[..count])).expect("Could not create Utf8 Url String");
        let path = url.split(':').nth(1).expect("Could not get path from url");

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

        Ok((display_file, width, height))
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        match display_fd_map(width, height, &mut self.display_file) {
            Ok(offscreen) => {
                self.offscreen = offscreen;
            }
            Err(err) => {
                eprintln!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        unsafe {
            libredox::call::write(
                self.display_file.as_raw_fd() as usize,
                slice::from_raw_parts(
                    sync_rects.as_ptr() as *const u8,
                    sync_rects.len() * mem::size_of::<Damage>(),
                ),
            )
            .unwrap();
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
