extern crate ransid;

use std::collections::{BTreeSet, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::{cmp, ptr};

use graphics_ipc::legacy::Damage;
use orbclient::FONT;

pub struct DisplayMap {
    pub offscreen: *mut [u32],
    pub width: usize,
    pub height: usize,
}

pub struct TextScreen {
    console: ransid::Console,
    changed: BTreeSet<usize>,
}

impl TextScreen {
    pub fn new() -> TextScreen {
        TextScreen {
            // Width and height will be filled in on the next write to the console
            console: ransid::Console::new(0, 0),
            changed: BTreeSet::new(),
        }
    }

    /// Draw a rectangle
    fn rect(map: &mut DisplayMap, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let start_y = cmp::min(map.height, y);
        let end_y = cmp::min(map.height, y + h);

        let start_x = cmp::min(map.width, x);
        let len = cmp::min(map.width, x + w) - start_x;

        let mut offscreen_ptr = map.offscreen as *mut u8 as usize;

        let stride = map.width * 4;

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
    fn invert(map: &mut DisplayMap, x: usize, y: usize, w: usize, h: usize) {
        let start_y = cmp::min(map.height, y);
        let end_y = cmp::min(map.height, y + h);

        let start_x = cmp::min(map.width, x);
        let len = cmp::min(map.width, x + w) - start_x;

        let mut offscreen_ptr = map.offscreen as *mut u8 as usize;

        let stride = map.width * 4;

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
    fn char(
        map: &mut DisplayMap,
        x: usize,
        y: usize,
        character: char,
        color: u32,
        _bold: bool,
        _italic: bool,
    ) {
        if x + 8 <= map.width && y + 16 <= map.height {
            let mut dst = map.offscreen as *mut u8 as usize + (y * map.width + x) * 4;

            let font_i = 16 * (character as usize);
            if font_i + 16 <= FONT.len() {
                for row in 0..16 {
                    let row_data = FONT[font_i + row];
                    for col in 0..8 {
                        if (row_data >> (7 - col)) & 1 == 1 {
                            unsafe {
                                *((dst + col * 4) as *mut u32) = color;
                            }
                        }
                    }
                    dst += map.width * 4;
                }
            }
        }
    }
}

impl TextScreen {
    pub fn write(
        &mut self,
        map: &mut DisplayMap,
        buf: &[u8],
        input: &mut VecDeque<u8>,
    ) -> Vec<Damage> {
        self.console.resize(map.width / 8, map.height / 16);
        if self.console.state.x > self.console.state.w {
            self.console.state.x = self.console.state.w;
        }
        if self.console.state.y > self.console.state.h {
            self.console.state.y = self.console.state.h;
        }

        if self.console.state.cursor
            && self.console.state.x < self.console.state.w
            && self.console.state.y < self.console.state.h
        {
            let x = self.console.state.x;
            let y = self.console.state.y;
            Self::invert(map, x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        self.console.write(buf, |event| match event {
            ransid::Event::Char {
                x,
                y,
                c,
                color,
                bold,
                ..
            } => {
                Self::char(map, x * 8, y * 16, c, color.as_rgb(), bold, false);
                self.changed.insert(y);
            }
            ransid::Event::Input { data } => input.extend(data),
            ransid::Event::Rect { x, y, w, h, color } => {
                Self::rect(map, x * 8, y * 16, w * 8, h * 16, color.as_rgb());
                for y2 in y..y + h {
                    self.changed.insert(y2);
                }
            }
            ransid::Event::ScreenBuffer { .. } => (),
            ransid::Event::Move {
                from_x,
                from_y,
                to_x,
                to_y,
                w,
                h,
            } => {
                let width = map.width;
                let pixels = unsafe { &mut *map.offscreen };

                for raw_y in 0..h {
                    let y = if from_y > to_y { raw_y } else { h - raw_y - 1 };

                    for pixel_y in 0..16 {
                        {
                            let off_from = ((from_y + y) * 16 + pixel_y) * width + from_x * 8;
                            let off_to = ((to_y + y) * 16 + pixel_y) * width + to_x * 8;
                            let len = w * 8;

                            if off_from + len <= pixels.len() && off_to + len <= pixels.len() {
                                unsafe {
                                    let data_ptr = pixels.as_mut_ptr() as *mut u32;
                                    ptr::copy(
                                        data_ptr.offset(off_from as isize),
                                        data_ptr.offset(off_to as isize),
                                        len,
                                    );
                                }
                            }
                        }
                    }

                    self.changed.insert(to_y + y);
                }
            }
            ransid::Event::Resize { .. } => (),
            ransid::Event::Title { .. } => (),
        });

        if self.console.state.cursor
            && self.console.state.x < self.console.state.w
            && self.console.state.y < self.console.state.h
        {
            let x = self.console.state.x;
            let y = self.console.state.y;
            Self::invert(map, x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        let width = map.width.try_into().unwrap();
        let mut damage: Vec<Damage> = vec![];
        let mut last_change = usize::MAX - 1;
        for &change in &self.changed {
            if change == last_change + 1 {
                damage.last_mut().unwrap().height += 16;
            } else {
                damage.push(Damage {
                    x: 0,
                    y: i32::try_from(change).unwrap() * 16,
                    width,
                    height: 16,
                });
            }
            last_change = change;
        }

        self.changed.clear();

        damage
    }
}