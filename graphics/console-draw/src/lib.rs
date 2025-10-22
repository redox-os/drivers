extern crate ransid;

use std::collections::VecDeque;
use std::convert::{TryFrom, TryInto};
use std::{cmp, ptr};

use graphics_ipc::v1::Damage;
use orbclient::FONT;

pub struct DisplayMap {
    pub offscreen: *mut [u32],
    pub width: usize,
    pub height: usize,
}

pub struct TextScreen {
    console: ransid::Console,
}

impl TextScreen {
    pub fn new() -> TextScreen {
        TextScreen {
            // Width and height will be filled in on the next write to the console
            console: ransid::Console::new(0, 0),
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
    pub fn write(&mut self, map: &mut DisplayMap, buf: &[u8], input: &mut VecDeque<u8>) -> Damage {
        let mut min_changed = map.height;
        let mut max_changed = 0;
        let mut line_changed = |line| {
            if line < min_changed {
                min_changed = line;
            }
            if line > max_changed {
                max_changed = line;
            }
        };

        self.console.resize(map.width / 8, map.height / 16);
        if self.console.state.x >= self.console.state.w {
            self.console.state.x = self.console.state.w - 1;
        }
        if self.console.state.y >= self.console.state.h {
            self.console.state.y = self.console.state.h - 1;
        }

        if self.console.state.cursor
            && self.console.state.x < self.console.state.w
            && self.console.state.y < self.console.state.h
        {
            let x = self.console.state.x;
            let y = self.console.state.y;
            Self::invert(map, x * 8, y * 16, 8, 16);
            line_changed(y);
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
                line_changed(y);
            }
            ransid::Event::Input { data } => input.extend(data),
            ransid::Event::Rect { x, y, w, h, color } => {
                Self::rect(map, x * 8, y * 16, w * 8, h * 16, color.as_rgb());
                for y2 in y..y + h {
                    line_changed(y2);
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

                    line_changed(to_y + y);
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
            line_changed(y);
        }

        let width = map.width.try_into().unwrap();
        let damage = Damage {
            x: 0,
            y: u32::try_from(min_changed).unwrap() * 16,
            width,
            height: u32::try_from(max_changed.saturating_sub(min_changed) + 1).unwrap() * 16,
        };

        damage
    }

    pub fn resize(&mut self, old_map: &mut DisplayMap, new_map: &mut DisplayMap) {
        // FIXME fold row when target is narrower and maybe unfold when it is wider
        fn copy_row(
            old_map: &mut DisplayMap,
            new_map: &mut DisplayMap,
            from_row: usize,
            to_row: usize,
        ) {
            for x in 0..cmp::min(old_map.width, new_map.width) {
                let old_idx = from_row * old_map.width + x;
                let new_idx = to_row * new_map.width + x;
                unsafe {
                    (*new_map.offscreen)[new_idx] = (*old_map.offscreen)[old_idx];
                }
            }
        }

        if new_map.height >= old_map.height {
            for row in 0..old_map.height {
                copy_row(old_map, new_map, row, row);
            }
        } else {
            let deleted_rows = (old_map.height - new_map.height).div_ceil(16);
            for row in 0..new_map.height {
                if row + (deleted_rows + 1) * 16 >= old_map.height {
                    break;
                }
                copy_row(old_map, new_map, row + deleted_rows * 16, row);
            }
            self.console.state.y = self.console.state.y.saturating_sub(deleted_rows);
        }
    }
}

pub struct TextBuffer {
    pub lines: VecDeque<Vec<u8>>,
    pub lines_max: usize,
}

impl TextBuffer {
    pub fn new(max: usize) -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(Vec::new());
        Self {
            lines,
            lines_max: max,
        }
    }
    pub fn write(&mut self, buf: &[u8]) {
        if buf.is_empty() {
            return;
        }

        for &byte in buf {
            self.lines.back_mut().unwrap().push(byte);

            if byte == b'\n' {
                self.lines.push_back(Vec::new());
            }
        }

        let max_len = self.lines_max;
        while self.lines.len() > max_len {
            self.lines.pop_front();
        }
    }
}
