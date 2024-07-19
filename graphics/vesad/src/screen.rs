use std::collections::VecDeque;
use std::convert::TryInto;
use std::{cmp, mem, ptr, slice};

use orbclient::{Event, ResizeEvent};
use syscall::error::*;

use crate::display::OffscreenBuffer;
use crate::framebuffer::FrameBuffer;

// Keep synced with orbital
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct SyncRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub struct GraphicScreen {
    pub width: usize,
    pub height: usize,
    pub offscreen: OffscreenBuffer,
    pub input: VecDeque<Event>,
    pub sync_rects: Vec<SyncRect>,
}

impl GraphicScreen {
    pub fn new(width: usize, height: usize) -> GraphicScreen {
        GraphicScreen {
            width,
            height,
            offscreen: OffscreenBuffer::new(width * height),
            input: VecDeque::new(),
            sync_rects: Vec::new(),
        }
    }
}

impl GraphicScreen {
    pub fn resize(&mut self, width: usize, height: usize) {
        //TODO: Fix issue with mapped screens

        if width != self.width || height != self.height {
            println!("Resize display to {}, {}", width, height);

            let size = width * height;
            let mut offscreen = OffscreenBuffer::new(size);

            let mut old_ptr = self.offscreen.as_ptr();
            let mut new_ptr = offscreen.as_mut_ptr();

            for _y in 0..cmp::min(height, self.height) {
                unsafe {
                    ptr::copy(old_ptr, new_ptr, cmp::min(width, self.width));
                    if width > self.width {
                        ptr::write_bytes(
                            new_ptr.offset(self.width as isize),
                            0,
                            width - self.width,
                        );
                    }
                    old_ptr = old_ptr.offset(self.width as isize);
                    new_ptr = new_ptr.offset(width as isize);
                }
            }

            if height > self.height {
                for _y in self.height..height {
                    unsafe {
                        ptr::write_bytes(new_ptr, 0, width);
                        new_ptr = new_ptr.offset(width as isize);
                    }
                }
            }

            self.width = width;
            self.height = height;

            self.offscreen = offscreen;
        } else {
            println!("Display is already {}, {}", width, height);
        };

        self.input.push_back(
            ResizeEvent {
                width: width as u32,
                height: height as u32,
            }
            .to_event(),
        );
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;

        let event_buf = unsafe {
            slice::from_raw_parts_mut(
                buf.as_mut_ptr() as *mut Event,
                buf.len() / mem::size_of::<Event>(),
            )
        };

        while i < event_buf.len() && !self.input.is_empty() {
            event_buf[i] = self.input.pop_front().unwrap();
            i += 1;
        }

        Ok(i * mem::size_of::<Event>())
    }

    pub fn can_read(&self) -> Option<usize> {
        if self.input.is_empty() {
            None
        } else {
            Some(self.input.len() * mem::size_of::<Event>())
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let sync_rects = unsafe {
            slice::from_raw_parts(
                buf.as_ptr() as *const SyncRect,
                buf.len() / mem::size_of::<SyncRect>(),
            )
        };

        self.sync_rects.extend_from_slice(sync_rects);

        Ok(sync_rects.len() * mem::size_of::<SyncRect>())
    }

    pub fn sync(&mut self, framebuffer: &mut FrameBuffer) {
        for sync_rect in self.sync_rects.drain(..) {
            let x = sync_rect.x.try_into().unwrap_or(0);
            let y = sync_rect.y.try_into().unwrap_or(0);
            let w = sync_rect.w.try_into().unwrap_or(0);
            let h = sync_rect.h.try_into().unwrap_or(0);

            let start_y = cmp::min(self.height, y);
            let end_y = cmp::min(self.height, y + h);

            let start_x = cmp::min(self.width, x);
            let row_pixel_count = cmp::min(self.width, x + w) - start_x;

            let mut offscreen_ptr = self.offscreen.as_mut_ptr();
            let mut onscreen_ptr = framebuffer.onscreen as *mut u32; // FIXME use as_mut_ptr once stable

            unsafe {
                offscreen_ptr = offscreen_ptr.add(y * self.width + start_x);
                onscreen_ptr = onscreen_ptr.add(y * framebuffer.stride + start_x);

                let mut rows = end_y - start_y;
                while rows > 0 {
                    ptr::copy(offscreen_ptr, onscreen_ptr, row_pixel_count);
                    offscreen_ptr = offscreen_ptr.add(self.width);
                    onscreen_ptr = onscreen_ptr.add(framebuffer.stride);
                    rows -= 1;
                }
            }
        }
    }

    pub fn redraw(&mut self, framebuffer: &mut FrameBuffer) {
        let width = self.width.try_into().unwrap();
        let height = self.height.try_into().unwrap();
        self.sync_rects.push(SyncRect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        });
        self.sync(framebuffer);
    }
}
