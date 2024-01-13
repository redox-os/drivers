use std::alloc::{self, Layout};
use std::{cmp, ptr};
use std::ptr::NonNull;

use orbclient::FONT;

pub struct OffscreenBuffer {
    ptr: NonNull<[u32]>,
}

impl OffscreenBuffer {
    #[inline]
    fn layout(len: usize) -> Layout {
        // optimizes to an integer mul
        Layout::array::<u32>(len).unwrap().align_to(4096).unwrap()
    }

    #[inline]
    fn new(len: usize) -> Self {
        let layout = Self::layout(len);
        let ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = ptr::slice_from_raw_parts_mut(ptr.cast(), len);
        let ptr = NonNull::new(ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        OffscreenBuffer { ptr }
    }
}
impl Drop for OffscreenBuffer {
    fn drop(&mut self) {
        let layout = Self::layout(self.ptr.len());
        unsafe { alloc::dealloc(self.ptr.as_ptr().cast(), layout) };
    }
}
impl std::ops::Deref for OffscreenBuffer {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        unsafe { self.ptr.as_ref() }
    }
}
impl std::ops::DerefMut for OffscreenBuffer {
    fn deref_mut(&mut self) -> &mut [u32] {
        unsafe { self.ptr.as_mut() }
    }
}

/// A display
pub struct Display {
    pub width: usize,
    pub height: usize,
    pub offscreen: OffscreenBuffer,
}

impl Display {
    pub fn new(width: usize, height: usize) -> Display {
        let size = width * height;
        let offscreen = OffscreenBuffer::new(size);
        Display {
            width,
            height,
            offscreen,
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if width != self.width || height != self.height {
            println!("Resize display to {}, {}", width, height);

            let size = width * height;
            let mut offscreen = OffscreenBuffer::new(size);

            {
                let mut old_ptr = self.offscreen.as_ptr();
                let mut new_ptr = offscreen.as_mut_ptr();

                for _y in 0..cmp::min(height, self.height) {
                    unsafe {
                        ptr::copy(
                            old_ptr as *const u8,
                            new_ptr as *mut u8,
                            cmp::min(width, self.width) * 4
                        );
                        if width > self.width {
                            ptr::write_bytes(
                                new_ptr.offset(self.width as isize),
                                0,
                                width - self.width
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
            }

            self.width = width;
            self.height = height;

            self.offscreen = offscreen;
        } else {
            println!("Display is already {}, {}", width, height);
        }
    }

    /// Draw a rectangle
    pub fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let start_y = cmp::min(self.height, y);
        let end_y = cmp::min(self.height, y + h);

        let start_x = cmp::min(self.width, x);
        let len = cmp::min(self.width, x + w) - start_x;

        let mut offscreen_ptr = self.offscreen.as_mut_ptr() as usize;

        let stride = self.width * 4;

        let offset = y * stride + start_x * 4;
        offscreen_ptr += offset;

        let mut rows = end_y - start_y;
        while rows > 0 {
            for i in 0..len {
                unsafe {
                    *(offscreen_ptr as *mut u32).add(i) = color;
                }
            }
            offscreen_ptr += stride;
            rows -= 1;
        }
    }

    /// Invert a rectangle
    pub fn invert(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let start_y = cmp::min(self.height, y);
        let end_y = cmp::min(self.height, y + h);

        let start_x = cmp::min(self.width, x);
        let len = cmp::min(self.width, x + w) - start_x;

        let mut offscreen_ptr = self.offscreen.as_mut_ptr() as usize;

        let stride = self.width * 4;

        let offset = y * stride + start_x * 4;
        offscreen_ptr += offset;

        let mut rows = end_y - start_y;
        while rows > 0 {
            let mut row_ptr = offscreen_ptr;
            let mut cols = len;
            while cols > 0 {
                unsafe {
                    let color = *(row_ptr as *mut u32);
                    *(row_ptr as *mut u32) = !color;
                }
                row_ptr += 4;
                cols -= 1;
            }
            offscreen_ptr += stride;
            rows -= 1;
        }
    }

    /// Draw a character
    pub fn char(&mut self, x: usize, y: usize, character: char, color: u32, _bold: bool, _italic: bool) {
        if x + 8 <= self.width && y + 16 <= self.height {
            let mut dst = self.offscreen.as_mut_ptr() as usize + (y * self.width + x) * 4;

            let font_i = 16 * (character as usize);
            if font_i + 16 <= FONT.len() {
                for row in 0..16 {
                    let row_data = FONT[font_i + row];
                    for col in 0..8 {
                        if (row_data >> (7 - col)) & 1 == 1 {
                            unsafe { *((dst + col * 4) as *mut u32)  = color; }
                        }
                    }
                    dst += self.width * 4;
                }
            }
        }
    }

    /// Copy from offscreen to onscreen
    pub fn sync(&mut self, x: usize, y: usize, w: usize, h: usize, onscreen: &mut [u32], stride: usize) {
        let start_y = cmp::min(self.height, y);
        let end_y = cmp::min(self.height, y + h);

        let start_x = cmp::min(self.width, x);
        let len = (cmp::min(self.width, x + w) - start_x) * 4;

        let mut offscreen_ptr = self.offscreen.as_mut_ptr() as usize;
        let mut onscreen_ptr = onscreen.as_mut_ptr() as usize;

        offscreen_ptr += (y * self.width + start_x) * 4;
        onscreen_ptr += (y * stride + start_x) * 4;

        let mut rows = end_y - start_y;
        while rows > 0 {
            unsafe {
                ptr::copy(
                    offscreen_ptr as *const u8,
                    onscreen_ptr as *mut u8,
                    len
                );
            }
            offscreen_ptr += self.width * 4;
            onscreen_ptr += stride * 4;
            rows -= 1;
        }
    }
}
