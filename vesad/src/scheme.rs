use std::collections::BTreeMap;
use std::{mem, slice, str};

use orbclient::{Keymap, Event, EventOption};
use orbclient::keycode::*;
use syscall::{Result, Error, EACCES, EBADF, ENOENT, SchemeMut};

use display::Display;
use screen::{Screen, GraphicScreen, TextScreen};

pub struct DisplayScheme {
    width: usize,
    height: usize,
    active: usize,
    keymap: Keymap,
    pub screens: BTreeMap<usize, Box<Screen>>
}

impl DisplayScheme {
    pub fn new(width: usize, height: usize, onscreen: usize, spec: &[bool]) -> DisplayScheme {
        let mut screens: BTreeMap<usize, Box<Screen>> = BTreeMap::new();

        let mut screen_i = 1;
        for &screen_type in spec.iter() {
            if screen_type {
                screens.insert(screen_i, Box::new(GraphicScreen::new(Display::new(width, height, onscreen))));
            } else {
                screens.insert(screen_i, Box::new(TextScreen::new(Display::new(width, height, onscreen))));
            }
            screen_i += 1;
        }

        DisplayScheme {
            width: width,
            height: height,
            active: 1,
            keymap: Keymap::default(),
            screens: screens
        }
    }

    pub fn will_block(&self, id: usize) -> bool {
        if let Some(screen) = self.screens.get(&id) {
            screen.will_block()
        } else {
            false
        }
    }
}

impl SchemeMut for DisplayScheme {
    fn open(&mut self, path: &[u8], _flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if path == b"input" {
            if uid == 0 {
                Ok(0)
            } else {
                Err(Error::new(EACCES))
            }
        } else if path == b"keymap"{
            Ok(4000000000)
        } else {
            let path_str = str::from_utf8(path).unwrap_or("").trim_matches('/');
            let mut parts = path_str.split('/');
            let id = parts.next().unwrap_or("").parse::<usize>().unwrap_or(0);
            if self.screens.contains_key(&id) {
                for cmd in parts {
                    if cmd == "activate" {
                        self.active = id;
                    }
                }
                Ok(id)
            } else {
                Err(Error::new(ENOENT))
            }
        }
    }

    fn dup(&mut self, id: usize, _buf: &[u8]) -> Result<usize> {
        Ok(id)
    }

    fn fevent(&mut self, id: usize, flags: usize) -> Result<usize> {
        if let Some(mut screen) = self.screens.get_mut(&id) {
            screen.event(flags).and(Ok(id))
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fmap(&mut self, id: usize, offset: usize, size: usize) -> Result<usize> {
        if let Some(screen) = self.screens.get(&id) {
            screen.map(offset, size)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let path_str = if id == 0 {
            format!("display:input/{}/{}", self.width, self.height)
        } else if id == 4000000000 {
            format!("display:keymap")
        } else if let Some(screen) = self.screens.get(&id) {
            format!("display:{}/{}/{}", id, screen.width(), screen.height())
        } else {
            return Err(Error::new(EBADF));
        };

        let path = path_str.as_bytes();

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(i)
    }

    fn fsync(&mut self, id: usize) -> Result<usize> {
        if let Some(mut screen) = self.screens.get_mut(&id) {
            if id == self.active {
                screen.sync();
            }
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        if let Some(mut screen) = self.screens.get_mut(&id) {
            screen.read(buf)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        if id == 0 {
            if buf.len() == 1 && buf[0] >= 0xF4 {
                let new_active = (buf[0] - 0xF4) as usize + 1;
                if let Some(mut screen) = self.screens.get_mut(&new_active) {
                    self.active = new_active;
                    screen.redraw();
                }
                Ok(1)
            } else {
                let events = unsafe { slice::from_raw_parts(buf.as_ptr() as *const Event, buf.len()/mem::size_of::<Event>()) };

                for event in events.iter() {
                    let mut new_active_opt = None;
                    match event.to_option() {
                        EventOption::Key(key_event) => match key_event.keycode {
                            f @ KC_F1 ... KC_F10 => { // F1 through F10
                                new_active_opt = Some((f - KC_F1 + 1) as usize);
                            },
                            KC_F11 => { // F11
                                new_active_opt = Some(11);
                            },
                            KC_F12 => { // F12
                                new_active_opt = Some(12);
                            },
                            _ => ()
                        },
                        EventOption::Resize(resize_event) => {
                            println!("Resizing to {}, {}", resize_event.width, resize_event.height);
                            for (screen_id, screen) in self.screens.iter_mut() {
                                screen.resize(resize_event.width as usize, resize_event.height as usize);
                                if *screen_id == self.active {
                                    screen.redraw();
                                }
                            }
                        },
                        _ => ()
                    };

                    if let Some(new_active) = new_active_opt {
                        if let Some(mut screen) = self.screens.get_mut(&new_active) {
                            self.active = new_active;
                            screen.redraw();
                        }
                    } else {
                        if let Some(mut screen) = self.screens.get_mut(&self.active) {
                            // Apply keymap
                            let event_opt = event.to_option();
                            let event = if let EventOption::Key(mut key_event) = event_opt {
                                key_event.character = match key_event.keycode {
                                    c @ 0 ... 58 => self.keymap.get_char(c, key_event.modifiers),
                                    _ => '\0',
                                };

                                // from old code:
                                // c @ 0 ... 58 => event.character = self.keymap.get_char(c, self.shift_key, self.alt_gr_key),
                                key_event.to_event()
                            } else {
                                *event
                            };


                            // Pass on the event to the active screen
                            screen.input(&event);
                        }
                    }
                }

                Ok(events.len() * mem::size_of::<Event>())
            }
        } else if id == 4000000000 {
            let keymap = Keymap::from_file(str::from_utf8(buf).unwrap());
            self.keymap = keymap?;
            Ok(buf.len())
        } else if let Some(mut screen) = self.screens.get_mut(&id) {
            screen.write(buf, id == self.active)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn seek(&mut self, id: usize, pos: usize, whence: usize) -> Result<usize> {
        if let Some(mut screen) = self.screens.get_mut(&id) {
            screen.seek(pos, whence)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn close(&mut self, _id: usize) -> Result<usize> {
        Ok(0)
    }
}
