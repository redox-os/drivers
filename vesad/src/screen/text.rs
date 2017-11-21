extern crate ransid;

use std::collections::{BTreeSet, VecDeque};
use std::ptr;

use orbclient::{Event, EventOption};
use syscall::error::*;

use display::Display;
use screen::Screen;

pub struct TextScreen {
    pub console: ransid::Console,
    pub display: Display,
    pub changed: BTreeSet<usize>,
    pub ctrl: bool,
    pub input: VecDeque<u8>,
    pub requested: usize
}

impl TextScreen {
    pub fn new(display: Display) -> TextScreen {
        TextScreen {
            console: ransid::Console::new(display.width/8, display.height/16),
            display: display,
            changed: BTreeSet::new(),
            ctrl: false,
            input: VecDeque::new(),
            requested: 0
        }
    }
}

impl Screen for TextScreen {
    fn width(&self) -> usize {
        self.console.state.w
    }

    fn height(&self) -> usize {
        self.console.state.h
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.display.resize(width, height);
        self.console.state.w = width / 8;
        self.console.state.h = height / 16;
    }

    fn event(&mut self, flags: usize) -> Result<usize> {
        self.requested = flags;
        Ok(0)
    }

    fn map(&self, _offset: usize, _size: usize) -> Result<usize> {
        Err(Error::new(EBADF))
    }

    fn input(&mut self, event: &Event) {
        let mut buf = vec![];

        match event.to_option() {
            EventOption::Key(key_event) => {
                if key_event.scancode == 0x1D {
                    self.ctrl = key_event.pressed;
                } else if key_event.pressed {
                    match key_event.scancode {
                        0x0E => { // Backspace
                            buf.extend_from_slice(b"\x7F");
                        },
                        0x47 => { // Home
                            buf.extend_from_slice(b"\x1B[H");
                        },
                        0x48 => { // Up
                            buf.extend_from_slice(b"\x1B[A");
                        },
                        0x49 => { // Page up
                            buf.extend_from_slice(b"\x1B[5~");
                        },
                        0x4B => { // Left
                            buf.extend_from_slice(b"\x1B[D");
                        },
                        0x4D => { // Right
                            buf.extend_from_slice(b"\x1B[C");
                        },
                        0x4F => { // End
                            buf.extend_from_slice(b"\x1B[F");
                        },
                        0x50 => { // Down
                            buf.extend_from_slice(b"\x1B[B");
                        },
                        0x51 => { // Page down
                            buf.extend_from_slice(b"\x1B[6~");
                        },
                        0x52 => { // Insert
                            buf.extend_from_slice(b"\x1B[2~");
                        },
                        0x53 => { // Delete
                            buf.extend_from_slice(b"\x1B[3~");
                        },
                        _ => {
                            let c = match key_event.character {
                                c @ 'A' ... 'Z' if self.ctrl => ((c as u8 - b'A') + b'\x01') as char,
                                c @ 'a' ... 'z' if self.ctrl => ((c as u8 - b'a') + b'\x01') as char,
                                c => c
                            };

                            if c != '\0' {
                                buf.extend_from_slice(&[c as u8]);
                            }
                        }
                    }
                }
            },
            _ => () //TODO: Mouse in terminal
        }

        for &b in buf.iter() {
            self.input.push_back(b);
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;

        while i < buf.len() && ! self.input.is_empty() {
            buf[i] = self.input.pop_front().unwrap();
            i += 1;
        }

        Ok(i)
    }

    fn can_read(&self) -> Option<usize> {
        if self.input.is_empty() {
            None
        } else {
            Some(self.input.len())
        }
    }

    fn write(&mut self, buf: &[u8], sync: bool) -> Result<usize> {
        if self.console.state.cursor && self.console.state.x < self.console.state.w && self.console.state.y < self.console.state.h {
            let x = self.console.state.x;
            let y = self.console.state.y;
            self.display.invert(x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        {
            let display = &mut self.display;
            let changed = &mut self.changed;
            let input = &mut self.input;
            self.console.write(buf, |event| {
                match event {
                    ransid::Event::Char { x, y, c, color, bold, .. } => {
                        display.char(x * 8, y * 16, c, color.as_rgb(), bold, false);
                        changed.insert(y);
                    },
                    ransid::Event::Input { data } => {
                        input.extend(data);
                    },
                    ransid::Event::Rect { x, y, w, h, color } => {
                        display.rect(x * 8, y * 16, w * 8, h * 16, color.as_rgb());
                        for y2 in y..y + h {
                            changed.insert(y2);
                        }
                    },
                    ransid::Event::ScreenBuffer { .. } => (),
                    ransid::Event::Move {from_x, from_y, to_x, to_y, w, h } => {
                        let width = display.width;
                        let pixels = &mut display.offscreen;

                        for raw_y in 0..h {
                            let y = if from_y > to_y {
                                raw_y
                            } else {
                                h - raw_y - 1
                            };

                            for pixel_y in 0..16 {
                                {
                                    let off_from = ((from_y + y) * 16 + pixel_y) * width + from_x * 8;
                                    let off_to = ((to_y + y) * 16 + pixel_y) * width + to_x * 8;
                                    let len = w * 8;

                                    if off_from + len <= pixels.len() && off_to + len <= pixels.len() {
                                        unsafe {
                                            let data_ptr = pixels.as_mut_ptr() as *mut u32;
                                            ptr::copy(data_ptr.offset(off_from as isize), data_ptr.offset(off_to as isize), len);
                                        }
                                    }
                                }
                            }

                            changed.insert(to_y + y);
                        }
                    },
                    ransid::Event::Resize { .. } => (),
                    ransid::Event::Title { .. } => ()
                }
            });
        }

        if self.console.state.cursor && self.console.state.x < self.console.state.w && self.console.state.y < self.console.state.h {
            let x = self.console.state.x;
            let y = self.console.state.y;
            self.display.invert(x * 8, y * 16, 8, 16);
            self.changed.insert(y);
        }

        if sync {
            self.sync();
        }

        Ok(buf.len())
    }

    fn seek(&mut self, _pos: usize, _whence: usize) -> Result<usize> {
        Ok(0)
    }

    fn sync(&mut self) {
        let width = self.display.width;
        for change in self.changed.iter() {
            self.display.sync(0, change * 16, width, 16);
        }
        self.changed.clear();
    }

    fn redraw(&mut self) {
        let width = self.display.width;
        let height = self.display.height;
        self.display.sync(0, 0, width, height);
        self.changed.clear();
    }
}
