use std::alloc::{self, Layout};
use std::convert::TryInto;
use std::ptr::{self, NonNull};

use driver_graphics::{CursorFramebuffer, CursorPlane, Framebuffer, GraphicsAdapter};
use graphics_ipc::v1::Damage;
use syscall::PAGE_SIZE;

pub struct FbAdapter {
    pub framebuffers: Vec<FrameBuffer>,
}

pub enum VesadCursor {}

impl CursorFramebuffer for VesadCursor {}

impl GraphicsAdapter for FbAdapter {
    type Framebuffer = GraphicScreen;
    type Cursor = VesadCursor;

    fn display_count(&self) -> usize {
        self.framebuffers.len()
    }

    fn display_size(&self, display_id: usize) -> (u32, u32) {
        (
            self.framebuffers[display_id].width as u32,
            self.framebuffers[display_id].height as u32,
        )
    }

    fn create_dumb_framebuffer(&mut self, width: u32, height: u32) -> Self::Framebuffer {
        GraphicScreen::new(width as usize, height as usize)
    }

    fn map_dumb_framebuffer(&mut self, framebuffer: &Self::Framebuffer) -> *mut u8 {
        framebuffer.ptr.as_ptr().cast::<u8>()
    }

    fn update_plane(&mut self, display_id: usize, framebuffer: &Self::Framebuffer, damage: Damage) {
        framebuffer.sync(&mut self.framebuffers[display_id], damage)
    }

    fn supports_hw_cursor(&self) -> bool {
        false
    }

    fn create_cursor_framebuffer(&mut self) -> VesadCursor {
        unimplemented!("Vesad does not support this function");
    }

    fn map_cursor_framebuffer(&mut self, _cursor: &Self::Cursor) -> *mut u8 {
        unimplemented!("Vesad does not support this function");
    }

    fn handle_cursor(&mut self, _cursor: &CursorPlane<VesadCursor>, _dirty_fb: bool) {
        unimplemented!("Vesad does not support this function");
    }
}

pub struct FrameBuffer {
    pub onscreen: *mut [u32],
    pub phys: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl FrameBuffer {
    pub unsafe fn new(phys: usize, width: usize, height: usize, stride: usize) -> Self {
        let size = stride * height;
        let virt = common::physmap(
            phys,
            size * 4,
            common::Prot {
                read: true,
                write: true,
            },
            common::MemoryType::WriteCombining,
        )
        .expect("vesad: failed to map framebuffer") as *mut u32;

        let onscreen = ptr::slice_from_raw_parts_mut(virt, size);

        Self {
            onscreen,
            phys,
            width,
            height,
            stride,
        }
    }

    pub unsafe fn parse(var: &str) -> Option<Self> {
        fn parse_number(part: &str) -> Option<usize> {
            let (start, radix) = if part.starts_with("0x") {
                (2, 16)
            } else {
                (0, 10)
            };
            match usize::from_str_radix(&part[start..], radix) {
                Ok(ok) => Some(ok),
                Err(err) => {
                    eprintln!("vesad: failed to parse '{}': {}", part, err);
                    None
                }
            }
        }

        let mut parts = var.split(',');
        let phys = parse_number(parts.next()?)?;
        let width = parse_number(parts.next()?)?;
        let height = parse_number(parts.next()?)?;
        let stride = parse_number(parts.next()?)?;
        Some(Self::new(phys, width, height, stride))
    }
}

pub struct GraphicScreen {
    width: usize,
    height: usize,
    ptr: NonNull<[u32]>,
}

impl GraphicScreen {
    fn new(width: usize, height: usize) -> GraphicScreen {
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

impl Framebuffer for GraphicScreen {
    fn width(&self) -> u32 {
        self.width as u32
    }

    fn height(&self) -> u32 {
        self.height as u32
    }
}

impl GraphicScreen {
    fn sync(&self, framebuffer: &mut FrameBuffer, sync_rect: Damage) {
        let sync_rect = sync_rect.clip(
            self.width.try_into().unwrap(),
            self.height.try_into().unwrap(),
        );

        let start_x: usize = sync_rect.x.try_into().unwrap();
        let start_y: usize = sync_rect.y.try_into().unwrap();
        let w: usize = sync_rect.width.try_into().unwrap();
        let h: usize = sync_rect.height.try_into().unwrap();

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
