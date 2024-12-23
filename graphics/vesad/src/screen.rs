use std::collections::VecDeque;
use std::convert::TryInto;
use std::{cmp, mem, ptr, slice};

use inputd::Damage;
use orbclient::{Event, ResizeEvent};
use syscall::error::*;

use crate::display::OffscreenBuffer;
use crate::framebuffer::FrameBuffer;

pub struct GraphicScreen {
    pub width: usize,
    pub height: usize,
    pub offscreen: OffscreenBuffer,
    pub input: VecDeque<Event>,
}

impl GraphicScreen {
    pub fn new(width: usize, height: usize) -> GraphicScreen {
        GraphicScreen {
            width,
            height,
            offscreen: OffscreenBuffer::new(width * height),
            input: VecDeque::new(),
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

    pub fn can_read(&self) -> bool {
        !self.input.is_empty()
    }

    pub fn write(&mut self, buf: &[u8], framebuffer: Option<&mut FrameBuffer>) -> Result<usize> {
        let sync_rects = unsafe {
            slice::from_raw_parts(
                buf.as_ptr() as *const Damage,
                buf.len() / mem::size_of::<Damage>(),
            )
        };

        if let Some(framebuffer) = framebuffer {
            self.sync(framebuffer, sync_rects);
        }

        Ok(sync_rects.len() * mem::size_of::<Damage>())
    }

    pub fn sync(&mut self, framebuffer: &mut FrameBuffer, sync_rects: &[Damage]) {
        for sync_rect in sync_rects {
            let sync_rect = sync_rect.clip(
                self.height.try_into().unwrap(),
                self.width.try_into().unwrap(),
            );

            let start_x: usize = sync_rect.x.try_into().unwrap_or(0);
            let start_y: usize = sync_rect.y.try_into().unwrap_or(0);
            let w: usize = sync_rect.width.try_into().unwrap_or(0);
            let h: usize = sync_rect.height.try_into().unwrap_or(0);

            let end_y = start_y + h;

            let row_pixel_count = w;

            let mut offscreen_ptr = self.offscreen.as_mut_ptr();
            let mut onscreen_ptr = framebuffer.onscreen as *mut u32; // FIXME use as_mut_ptr once stable

            unsafe {
                offscreen_ptr = offscreen_ptr.add(start_y * self.width + start_x);
                onscreen_ptr = onscreen_ptr.add(start_y * framebuffer.stride + start_x);

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
        self.sync(
            framebuffer,
            &[Damage {
                x: 0,
                y: 0,
                width,
                height,
            }],
        );
    }
}
