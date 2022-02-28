#[cfg(feature="rusttype")]
extern crate rusttype;

use std::alloc::{Allocator, Global, Layout};
use std::{cmp, slice};
use std::ptr::NonNull;

use crate::primitive::{fast_set32, fast_copy};

#[cfg(feature="rusttype")]
use self::rusttype::{Font, FontCollection, Scale, point};

#[cfg(not(feature="rusttype"))]
use orbclient::FONT;

#[cfg(feature="rusttype")]
static FONT: &'static [u8] = include_bytes!("../res/DejaVuSansMono.ttf");
#[cfg(feature="rusttype")]
static FONT_BOLD: &'static [u8] = include_bytes!("../res/DejaVuSansMono-Bold.ttf");
#[cfg(feature="rusttype")]
static FONT_BOLD_ITALIC: &'static [u8] = include_bytes!("../res/DejaVuSansMono-BoldOblique.ttf");
#[cfg(feature="rusttype")]
static FONT_ITALIC: &'static [u8] = include_bytes!("../res/DejaVuSansMono-Oblique.ttf");

/// A display
pub struct Display {
    pub width: usize,
    pub height: usize,
    pub onscreen: &'static mut [u32],
    pub offscreen: &'static mut [u32],
    #[cfg(feature="rusttype")]
    pub font: Font<'static>,
    #[cfg(feature="rusttype")]
    pub font_bold: Font<'static>,
    #[cfg(feature="rusttype")]
    pub font_bold_italic: Font<'static>,
    #[cfg(feature="rusttype")]
    pub font_italic: Font<'static>
}

impl Display {
    #[cfg(not(feature="rusttype"))]
    pub fn new(width: usize, height: usize, onscreen: usize) -> Display {
        let size = width * height;

        let offscreen = unsafe {
            Global
                .allocate_zeroed(Layout::from_size_align_unchecked(size * 4, 4096))
                .expect("failed to allocate offscreen memory")
                .as_ptr()
        };
        Display {
            width: width,
            height: height,
            onscreen: unsafe { slice::from_raw_parts_mut(onscreen as *mut u32, size) },
            offscreen: unsafe { slice::from_raw_parts_mut(offscreen as *mut u32, size) }
        }
    }

    #[cfg(feature="rusttype")]
    pub fn new(width: usize, height: usize, onscreen: usize) -> Display {
        let size = width * height;
        let offscreen = unsafe {
            Global
                .allocate_zeroed(Layout::from_size_align_unchecked(size * 4, 4096))
                .expect("failed to allocate offscreen memory")
                .as_ptr()
        };
        Display {
            width: width,
            height: height,
            onscreen: unsafe { slice::from_raw_parts_mut(onscreen as *mut u32, size) },
            offscreen: unsafe { slice::from_raw_parts_mut(offscreen as *mut u32, size) },
            font: FontCollection::from_bytes(FONT).into_font().unwrap(),
            font_bold: FontCollection::from_bytes(FONT_BOLD).into_font().unwrap(),
            font_bold_italic: FontCollection::from_bytes(FONT_BOLD_ITALIC).into_font().unwrap(),
            font_italic: FontCollection::from_bytes(FONT_ITALIC).into_font().unwrap()
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if width != self.width || height != self.height {
            println!("Resize display to {}, {}", width, height);

            let size = width * height;
            let offscreen = unsafe {
                Global
                    .allocate_zeroed(Layout::from_size_align_unchecked(size * 4, 4096))
                    .expect("failed to allocate offscreen memory when resizing")
                    .as_ptr()
            };

            {
                let mut old_ptr = self.offscreen.as_ptr();
                let mut new_ptr = offscreen as *mut u32;

                for _y in 0..cmp::min(height, self.height) {
                    unsafe {
                        fast_copy(new_ptr as *mut u8, old_ptr as *const u8, cmp::min(width, self.width) * 4);
                        if width > self.width {
                            fast_set32(new_ptr.offset(self.width as isize), 0, width - self.width);
                        }
                        old_ptr = old_ptr.offset(self.width as isize);
                        new_ptr = new_ptr.offset(width as isize);
                    }
                }

                if height > self.height {
                    for _y in self.height..height {
                        unsafe {
                            fast_set32(new_ptr, 0, width);
                            new_ptr = new_ptr.offset(width as isize);
                        }
                    }
                }
            }

            self.width = width;
            self.height = height;

            let onscreen = self.onscreen.as_mut_ptr();
            self.onscreen = unsafe { slice::from_raw_parts_mut(onscreen, size) };

            unsafe { Global.deallocate(NonNull::new_unchecked(self.offscreen.as_mut_ptr() as *mut u8), Layout::from_size_align_unchecked(self.offscreen.len() * 4, 4096)) };
            self.offscreen = unsafe { slice::from_raw_parts_mut(offscreen as *mut u32, size) };
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
            unsafe {
                fast_set32(offscreen_ptr as *mut u32, color, len);
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
    #[cfg(not(feature="rusttype"))]
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

    /// Draw a character
    #[cfg(feature="rusttype")]
    pub fn char(&mut self, x: usize, y: usize, character: char, color: u32, bold: bool, italic: bool) {
        let width = self.width;
        let height = self.height;
        let offscreen = self.offscreen.as_mut_ptr() as usize;

        let font = if bold && italic {
            &self.font_bold_italic
        } else if bold {
            &self.font_bold
        } else if italic {
            &self.font_italic
        } else {
            &self.font
        };

        if let Some(glyph) = font.glyph(character){
            let scale = Scale::uniform(16.0);
            let v_metrics = font.v_metrics(scale);
            let point = point(0.0, v_metrics.ascent);
            let glyph = glyph.scaled(scale).positioned(point);
            if let Some(bb) = glyph.pixel_bounding_box() {
                glyph.draw(|off_x, off_y, v| {
                    let off_x = x + (off_x as i32 + bb.min.x) as usize;
                    let off_y = y + (off_y as i32 + bb.min.y) as usize;
                    // There's still a possibility that the glyph clips the boundaries of the bitmap
                    if off_x < width && off_y < height {
                        if v > 0.0 {
                            let f_a = (v * 255.0) as u32;
                            let f_r = (((color >> 16) & 0xFF) * f_a)/255;
                            let f_g = (((color >> 8) & 0xFF) * f_a)/255;
                            let f_b = ((color & 0xFF) * f_a)/255;

                            let offscreen_ptr = (offscreen + (off_y * width + off_x) * 4) as *mut u32;

                            let bg = unsafe { *offscreen_ptr };

                            let b_a = 255 - f_a;
                            let b_r = (((bg >> 16) & 0xFF) * b_a)/255;
                            let b_g = (((bg >> 8) & 0xFF) * b_a)/255;
                            let b_b = ((bg & 0xFF) * b_a)/255;

                            let c = ((f_r + b_r) << 16) | ((f_g + b_g) << 8) | (f_b + b_b);

                            unsafe { *offscreen_ptr = c; }
                        }
                    }
                });
            }
        }
    }

    /// Copy from offscreen to onscreen
    pub fn sync(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let start_y = cmp::min(self.height, y);
        let end_y = cmp::min(self.height, y + h);

        let start_x = cmp::min(self.width, x);
        let len = (cmp::min(self.width, x + w) - start_x) * 4;

        let mut offscreen_ptr = self.offscreen.as_mut_ptr() as usize;
        let mut onscreen_ptr = self.onscreen.as_mut_ptr() as usize;

        let stride = self.width * 4;

        let offset = y * stride + start_x * 4;
        offscreen_ptr += offset;
        onscreen_ptr += offset;

        let mut rows = end_y - start_y;
        while rows > 0 {
            unsafe {
                fast_copy(onscreen_ptr as *mut u8, offscreen_ptr as *const u8, len);
            }
            offscreen_ptr += stride;
            onscreen_ptr += stride;
            rows -= 1;
        }
    }
}

impl Drop for Display {
    #[cold]
    fn drop(&mut self) {
        unsafe {
            let offscreen = std::mem::replace(&mut self.offscreen, &mut []);

            let layout = Layout::from_size_align(offscreen.len() * 4, 4096).unwrap();

            Global.deallocate(
                NonNull::from(offscreen).cast::<u8>(),
                layout,
            );
        }
    }
}
