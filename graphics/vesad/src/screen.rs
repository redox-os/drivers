use std::alloc::{self, Layout};
use std::convert::TryInto;
use std::ptr::{self, NonNull};

use driver_graphics::Resource;
use graphics_ipc::legacy::Damage;
use syscall::PAGE_SIZE;

use crate::framebuffer::FrameBuffer;

pub struct GraphicScreen {
    pub width: usize,
    pub height: usize,
    ptr: NonNull<[u32]>,
}

impl GraphicScreen {
    pub fn new(width: usize, height: usize) -> GraphicScreen {
        let len = width * height;
        let layout = Self::layout(len);
        let ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = ptr::slice_from_raw_parts_mut(ptr.cast(), len);
        let ptr = NonNull::new(ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));

        GraphicScreen { width, height, ptr }
    }

    #[inline]
    fn layout(len: usize) -> Layout {
        // optimizes to an integer mul
        Layout::array::<u32>(len)
            .unwrap()
            .align_to(PAGE_SIZE)
            .unwrap()
    }
}

impl Drop for GraphicScreen {
    fn drop(&mut self) {
        let layout = Self::layout(self.ptr.len());
        unsafe { alloc::dealloc(self.ptr.as_ptr().cast(), layout) };
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
    pub fn ptr(&self) -> *mut u8 {
        self.ptr.as_ptr().cast::<u8>()
    }

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

            let mut offscreen_ptr = self.ptr.as_ptr() as *mut u32;
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
