extern crate ransid;

use std::collections::VecDeque;
use std::convert::TryInto;

use orbclient::{Event, EventOption};
use syscall::error::*;

use crate::display::Display;

pub struct TextScreen {
    pub display: Display,
    inner: console_draw::TextScreen,
    ctrl: bool,
    input: VecDeque<u8>,
}

impl TextScreen {
    pub fn new(display: Display) -> TextScreen {
        TextScreen {
            display,
            inner: console_draw::TextScreen::new(),
            ctrl: false,
            input: VecDeque::new(),
        }
    }

    pub fn handle_handoff(&mut self) {
        self.display.reopen_for_handoff();
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.display
            .resize(width.try_into().unwrap(), height.try_into().unwrap());
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

    pub fn can_read(&self) -> bool {
        !self.input.is_empty()
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
        let damage = self.inner.write(
            &mut console_draw::DisplayMap {
                offscreen: self.display.offscreen,
                width: self.display.width,
                height: self.display.height,
            },
            buf,
            &mut self.input,
        );

        self.display.sync_rects(damage);

        Ok(buf.len())
    }
}
