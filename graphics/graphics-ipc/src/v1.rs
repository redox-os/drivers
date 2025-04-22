use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::{cmp, io, mem, ptr, slice};

use libredox::flag;

/// A graphics handle using the v1 graphics API.
///
/// The v1 graphics API only allows a single framebuffer for each VT and supports neither page
/// flipping nor cursor planes.
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

    pub fn sync_rect(&self, sync_rect: Damage) -> io::Result<()> {
        let cmd_type: u32 = 0;
        let mut buf = Vec::with_capacity(4 + mem::size_of::<Damage>());
        buf.extend_from_slice(&cmd_type.to_le_bytes());
        buf.extend_from_slice(unsafe {
            slice::from_raw_parts(
                ptr::addr_of!(sync_rect).cast::<u8>(),
                mem::size_of::<Damage>(),
            )
        });

        libredox::call::write(self.file.as_raw_fd() as usize, &buf)?;
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

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub enum GraphicsCommand {
    UpdateDisplay(Damage),
    UpdateCursor(CursorDamage),
    CreateFramebuffer(CreateFramebuffer),
    SetFrontBuffer(usize),
    DestroyBuffer(usize),
}

impl GraphicsCommand {
    pub fn command_type(buf: &[u8]) -> GraphicsCommand {
        let command_type = u32::from_ne_bytes(buf[..4].try_into().unwrap());

        match command_type {
            0 => {
                assert_eq!(buf[4..].len(), mem::size_of::<Damage>());
                let damage = unsafe { *(buf[4..].as_ptr() as *const Damage) };
                GraphicsCommand::UpdateDisplay(damage)
            }
            1 => {
                assert_eq!(buf[4..].len(), mem::size_of::<CursorDamage>());
                let cursor_damage = unsafe { *(buf[4..].as_ptr() as *const CursorDamage) };
                GraphicsCommand::UpdateCursor(cursor_damage)
            }
            3 => {
                assert_eq!(buf[4..].len(), mem::size_of::<CreateFramebuffer>());
                let create_framebuffer =
                    unsafe { *(buf[4..].as_ptr() as *const CreateFramebuffer) };
                GraphicsCommand::CreateFramebuffer(create_framebuffer)
            }
            4 => {
                assert_eq!(buf[4..].len(), mem::size_of::<usize>());
                let buffer_index = unsafe { *(buf[4..].as_ptr() as *const usize) };
                GraphicsCommand::SetFrontBuffer(buffer_index)
            }
            5 => {
                assert_eq!(buf[4..].len(), mem::size_of::<usize>());
                let buffer_index = unsafe { *(buf[4..].as_ptr() as *const usize) };
                GraphicsCommand::DestroyBuffer(buffer_index)
            }
            _ => {
                panic!("Unknown command type: {command_type}");
            }
        }
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

// Keep synced with orbital's SyncRect
// Technically orbital uses i32 rather than u32, but values larger than i32::MAX
// would be a bug anyway.
#[derive(Debug, Copy, Clone)]
#[repr(packed)]
pub struct Damage {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Damage {
    #[must_use]
    pub fn clip(mut self, width: u32, height: u32) -> Self {
        // Clip damage
        let x2 = self.x + self.width;
        self.x = cmp::min(self.x, width);
        if x2 > width {
            self.width = width - self.x;
        }

        let y2 = self.y + self.height;
        self.y = cmp::min(self.y, height);
        if y2 > height {
            self.height = height - self.y;
        }
        self
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct CreateFramebuffer {
    pub width: u32,
    pub height: u32,
    pub id: usize,
}
