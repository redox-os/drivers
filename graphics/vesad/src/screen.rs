use std::convert::TryInto;
use std::ptr;

use driver_graphics::Resource;
use inputd::Damage;

use crate::display::OffscreenBuffer;
use crate::framebuffer::FrameBuffer;

pub struct GraphicScreen {
    pub width: usize,
    pub height: usize,
    pub offscreen: OffscreenBuffer,
}

impl GraphicScreen {
    pub fn new(width: usize, height: usize) -> GraphicScreen {
        GraphicScreen {
            width,
            height,
            offscreen: OffscreenBuffer::new(width * height),
        }
    }
}

impl Resource for GraphicScreen {
    fn width(&self) -> u32 {
        self.width as u32
    }

    fn height(&self) -> u32 {
        self.height as u32
    }
}

impl GraphicScreen {
    pub fn sync(&self, framebuffer: &mut FrameBuffer, sync_rects: &[Damage]) {
        for sync_rect in sync_rects {
            let sync_rect = sync_rect.clip(
                self.width.try_into().unwrap(),
                self.height.try_into().unwrap(),
            );

            let start_x: usize = sync_rect.x.try_into().unwrap_or(0);
            let start_y: usize = sync_rect.y.try_into().unwrap_or(0);
            let w: usize = sync_rect.width.try_into().unwrap_or(0);
            let h: usize = sync_rect.height.try_into().unwrap_or(0);

            let end_y = start_y + h;

            let row_pixel_count = w;

            let mut offscreen_ptr = self.offscreen.as_ptr();
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

    pub fn redraw(&self, framebuffer: &mut FrameBuffer) {
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
