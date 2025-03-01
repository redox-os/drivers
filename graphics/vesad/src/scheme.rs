use std::alloc::{self, Layout};
use std::convert::TryInto;
use std::ptr::{self, NonNull};

use driver_graphics::{GraphicsAdapter, Resource};
use graphics_ipc::legacy::Damage;
use syscall::PAGE_SIZE;

use crate::framebuffer::FrameBuffer;

pub struct FbAdapter {
    pub framebuffers: Vec<FrameBuffer>,
}

impl GraphicsAdapter for FbAdapter {
    type Resource = GraphicScreen;

    fn displays(&self) -> Vec<usize> {
        (0..self.framebuffers.len()).collect()
    }

    fn display_size(&self, display_id: usize) -> (u32, u32) {
        (
            self.framebuffers[display_id].width as u32,
            self.framebuffers[display_id].height as u32,
        )
    }

    fn create_resource(&mut self, width: u32, height: u32) -> Self::Resource {
        GraphicScreen::new(width as usize, height as usize)
    }

    fn map_resource(&mut self, resource: &Self::Resource) -> *mut u8 {
        resource.ptr.as_ptr().cast::<u8>()
    }

    fn set_scanout(&mut self, display_id: usize, resource: &Self::Resource) {
        self.flush_resource(display_id, resource, None);
    }

    fn flush_resource(
        &mut self,
        display_id: usize,
        resource: &Self::Resource,
        damage: Option<&[Damage]>,
    ) {
        if let Some(damage) = damage {
            resource.sync(&mut self.framebuffers[display_id], damage)
        } else {
            let framebuffer: &mut FrameBuffer = &mut self.framebuffers[display_id];
            let width = resource.width.try_into().unwrap();
            let height = resource.height.try_into().unwrap();
            resource.sync(
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
}

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
    fn sync(&self, framebuffer: &mut FrameBuffer, sync_rects: &[Damage]) {
        for sync_rect in sync_rects {
            let sync_rect = sync_rect.clip(
                self.width.try_into().unwrap(),
                self.height.try_into().unwrap(),
            );

            let start_x: usize = sync_rect.x.try_into().unwrap_or(0);
            let start_y: usize = sync_rect.y.try_into().unwrap_or(0);
            let w: usize = sync_rect.width.try_into().unwrap_or(0);
            let h: usize = sync_rect.height.try_into().unwrap_or(0);

            let offscreen_ptr = self.ptr.as_ptr() as *mut u32;
            let onscreen_ptr = framebuffer.onscreen as *mut u32; // FIXME use as_mut_ptr once stable

            for row in start_y..start_y + h {
                unsafe {
                    ptr::copy(
                        offscreen_ptr.add(row * self.width + start_x),
                        onscreen_ptr.add(row * framebuffer.stride + start_x),
                        w,
                    );
                }
            }
        }
    }
}
