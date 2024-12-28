use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::{cmp, io, mem, ptr, slice};

use libredox::flag;

/// A graphics handle using the legacy graphics API.
///
/// The legacy graphics API only allows a single framebuffer for each VT and supports neither page
/// flipping nor cursor planes.
pub struct LegacyGraphicsHandle {
    file: File,
}

impl LegacyGraphicsHandle {
    pub fn from_file(file: File) -> io::Result<Self> {
        Ok(LegacyGraphicsHandle { file })
    }

    pub fn map_display(&self) -> io::Result<DisplayMap> {
        let mut buf: [u8; 4096] = [0; 4096];
        let count =
            libredox::call::fpath(self.file.as_raw_fd() as usize, &mut buf).unwrap_or_else(|e| {
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

        let display_ptr = unsafe {
            libredox::call::mmap(libredox::call::MmapArgs {
                fd: self.file.as_raw_fd() as usize,
                offset: 0,
                length: (width * height * 4),
                prot: flag::PROT_READ | flag::PROT_WRITE,
                flags: flag::MAP_SHARED,
                addr: core::ptr::null_mut(),
            })?
        };
        let offscreen = ptr::slice_from_raw_parts_mut(display_ptr as *mut u32, width * height);

        Ok(DisplayMap {
            offscreen,
            width,
            height,
        })
    }

    pub fn sync_full_screen(&self) -> io::Result<()> {
        libredox::call::fsync(self.file.as_raw_fd() as usize)?;
        Ok(())
    }

    pub fn sync_rects(&self, sync_rects: &[Damage]) -> io::Result<()> {
        libredox::call::write(self.file.as_raw_fd() as usize, unsafe {
            slice::from_raw_parts(
                sync_rects.as_ptr() as *const u8,
                sync_rects.len() * mem::size_of::<Damage>(),
            )
        })?;
        Ok(())
    }
}

pub struct DisplayMap {
    offscreen: *mut [u32],
    width: usize,
    height: usize,
}

impl DisplayMap {
    pub fn ptr(&self) -> *const [u32] {
        self.offscreen
    }

    pub fn ptr_mut(&mut self) -> *mut [u32] {
        self.offscreen
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

unsafe impl Send for DisplayMap {}
unsafe impl Sync for DisplayMap {}

impl Drop for DisplayMap {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.offscreen as *mut (), self.offscreen.len());
        }
    }
}

// Keep synced with orbital's SyncRect
#[derive(Debug, Copy, Clone)]
#[repr(packed)]
pub struct Damage {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Damage {
    #[must_use]
    pub fn clip(mut self, width: i32, height: i32) -> Self {
        // Clip damage
        self.x = cmp::min(self.x, width);
        if self.x + self.width > width {
            self.width -= cmp::min(self.width, width - self.x);
        }
        self.y = cmp::min(self.y, height);
        if self.y + self.height > height {
            self.height = cmp::min(self.height, height - self.y);
        }
        self
    }
}
