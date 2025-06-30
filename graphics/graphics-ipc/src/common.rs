use std::cmp;

// Keep synced with orbital's SyncRect
// Technically orbital uses i32 rather than u32, but values larger than i32::MAX
// would be a bug anyway.
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
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

pub struct DisplayMap {
    offscreen: *mut [u32],
    width: usize,
    height: usize,
}

impl DisplayMap {
    pub(crate) unsafe fn new(offscreen: *mut [u32], width: usize, height: usize) -> Self {
        DisplayMap {
            offscreen,
            width,
            height,
        }
    }

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
