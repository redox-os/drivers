extern crate ransid;

use std::collections::{BTreeSet, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::os::fd::AsRawFd;
use std::{cmp, ptr};

use orbclient::{Event, EventOption, FONT};
use syscall::error::*;

use crate::display::{Display, SyncRect};

pub struct TextScreen {
    console: ransid::Console,
    pub display: Display,
    changed: BTreeSet<usize>,
    ctrl: bool,
    input: VecDeque<u8>,
}

impl TextScreen {
    pub fn new(display: Display) -> TextScreen {
        TextScreen {
            console: ransid::Console::new(display.width / 8, display.height / 16),
            display,
            changed: BTreeSet::new(),
            ctrl: false,
            input: VecDeque::new(),
        }
    }

    /// Draw a rectangle
    fn rect(display: &mut Display, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let start_y = cmp::min(display.height, y);
        let end_y = cmp::min(display.height, y + h);

        let start_x = cmp::min(display.width, x);
        let len = cmp::min(display.width, x + w) - start_x;

        let mut offscreen_ptr = display.offscreen as *mut u8 as usize;

        let stride = display.width * 4;

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
    fn invert(display: &mut Display, x: usize, y: usize, w: usize, h: usize) {
        let start_y = cmp::min(display.height, y);
        let end_y = cmp::min(display.height, y + h);

        let start_x = cmp::min(display.width, x);
        let len = cmp::min(display.width, x + w) - start_x;

        let mut offscreen_ptr = display.offscreen as *mut u8 as usize;

        let stride = display.width * 4;

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
        display: &mut Display,
        x: usize,
        y: usize,
        character: char,
        color: u32,
        _bold: bool,
        _italic: bool,
    ) {
        if x + 8 <= display.width && y + 16 <= display.height {
            let mut dst = display.offscreen as *mut u8 as usize + (y * display.width + x) * 4;

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
                    dst += display.width * 4;
                }
            }
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.display
            .resize(width.try_into().unwrap(), height.try_into().unwrap());
        self.console.state.w = width / 8;
        self.console.state.h = height / 16;
    }

    pub fn input(&mut self, event: &Event) {
        let mut buf = vec![];

        match event.to_option() {
            EventOption::Key(key_event) => {
                if key_event.scancode == 0x1D {
                    self.ctrl = key_event.pressed;
                } else if key_event.pressed {
                    match key_event.scancode {
                        0x0E => {
                            // Backspace
                            buf.extend_from_slice(b"\x7F");
                        }
                        0x47 => {
                            // Home
                            buf.extend_from_slice(b"\x1B[H");
                        }
                        0x48 => {
                            // Up
                            buf.extend_from_slice(b"\x1B[A");
                        }
                        0x49 => {
                            // Page up
                            buf.extend_from_slice(b"\x1B[5~");
                        }
                        0x4B => {
                            // Left
                            buf.extend_from_slice(b"\x1B[D");
                        }
                        0x4D => {
                            // Right
                            buf.extend_from_slice(b"\x1B[C");
                        }
                        0x4F => {
                            // End
                            buf.extend_from_slice(b"\x1B[F");
                        }
                        0x50 => {
                            // Down
                            buf.extend_from_slice(b"\x1B[B");
                        }
                        0x51 => {
                            // Page down
                            buf.extend_from_slice(b"\x1B[6~");
                        }
                        0x52 => {
                            // Insert
                            buf.extend_from_slice(b"\x1B[2~");
                        }
                        0x53 => {
                            // Delete
                            buf.extend_from_slice(b"\x1B[3~");
                        }
                        _ => {
                            let c = match key_event.character {
                                c @ 'A'..='Z' if self.ctrl => ((c as u8 - b'A') + b'\x01') as char,
                                c @ 'a'..='z' if self.ctrl => ((c as u8 - b'a') + b'\x01') as char,
                                c => c,
                            };

                            if c != '\0' {
                                let mut b = [0; 4];
                                buf.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                            }
                        }
                    }
                }
            }
            _ => (), //TODO: Mouse in terminal
        }

        for &b in buf.iter() {
            self.input.push_back(b);
        }
    }

    pub fn can_read(&self) -> Option<usize> {
        if self.input.is_empty() {
            None
        } else {
            Some(self.input.len())
        }
    }
}

impl TextScreen {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;

        while i < buf.len() && !self.input.is_empty() {
            buf[i] = self.input.pop_front().unwrap();
            i += 1;
        }

        Ok(i)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if self.console.state.cursor
            && self.console.state.x < self.console.state.w
            && self.console.state.y < self.console.state.h
        {
            let x = self.console.state.x;
            let y = self.console.state.y;
            Self::invert(&mut self.display, x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        {
            let display = &mut self.display;
            let changed = &mut self.changed;
            let input = &mut self.input;
            self.console.write(buf, |event| match event {
                ransid::Event::Char {
                    x,
                    y,
                    c,
                    color,
                    bold,
                    ..
                } => {
                    Self::char(display, x * 8, y * 16, c, color.as_rgb(), bold, false);
                    changed.insert(y);
                }
                ransid::Event::Input { data } => {
                    input.extend(data);
                }
                ransid::Event::Rect { x, y, w, h, color } => {
                    Self::rect(display, x * 8, y * 16, w * 8, h * 16, color.as_rgb());
                    for y2 in y..y + h {
                        changed.insert(y2);
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
                    let width = display.width;
                    let pixels = unsafe { &mut *display.offscreen };

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

                        changed.insert(to_y + y);
                    }
                }
                ransid::Event::Resize { .. } => (),
                ransid::Event::Title { .. } => (),
            });
        }

        if self.console.state.cursor
            && self.console.state.x < self.console.state.w
            && self.console.state.y < self.console.state.h
        {
            let x = self.console.state.x;
            let y = self.console.state.y;
            Self::invert(&mut self.display, x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        let width = self.display.width.try_into().unwrap();
        for &change in self.changed.iter() {
            self.display.sync_rect(SyncRect {
                x: 0,
                y: i32::try_from(change).unwrap() * 16,
                w: width,
                h: 16,
            })?;
        }
        self.changed.clear();
        libredox::call::fsync(self.display.display_file.as_raw_fd().as_raw_fd() as usize)?;

        Ok(buf.len())
    }
}
