use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::{io, mem, ptr, slice};

use libredox::flag;

pub use crate::common::Damage;
pub use crate::common::DisplayMap;

/// A graphics handle using the v1 graphics API.
///
/// The v1 graphics API only allows a single framebuffer for each VT, requires each display to be
/// handled separately and doesn't support page flipping.
///
/// This API is stable. No breaking changes are allowed to be made without a version bump.
pub struct V1GraphicsHandle {
    file: File,
}

impl V1GraphicsHandle {
    pub fn from_file(file: File) -> io::Result<Self> {
        Ok(V1GraphicsHandle { file })
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

        Ok(unsafe { DisplayMap::new(offscreen, width, height) })
    }

    pub fn sync_full_screen(&self) -> io::Result<()> {
        libredox::call::fsync(self.file.as_raw_fd() as usize)?;
        Ok(())
    }

    pub fn sync_rect(&self, sync_rect: Damage) -> io::Result<()> {
        libredox::call::write(self.file.as_raw_fd() as usize, unsafe {
            slice::from_raw_parts(
                ptr::addr_of!(sync_rect).cast::<u8>(),
                mem::size_of::<Damage>(),
            )
        })?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct CursorDamage {
    pub header: u32,
    pub x: i32,
    pub y: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub width: i32,
    pub height: i32,
    pub cursor_img_bytes: [u32; 4096],
}
