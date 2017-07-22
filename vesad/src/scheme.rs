use std::collections::BTreeMap;
use std::{mem, slice, str};

use orbclient::{Event, EventOption};
use syscall::{Result, Error, EACCES, EBADF, EINVAL, ENOENT, O_NONBLOCK, SchemeMut};

use display::Display;
use screen::{Screen, GraphicScreen, TextScreen};

#[derive(Clone)]
enum HandleKind {
    Input,
    Screen(usize),
}

#[derive(Clone)]
struct Handle {
    kind: HandleKind,
    flags: usize,
}

pub struct DisplayScheme {
    width: usize,
    height: usize,
    active: usize,
    pub screens: BTreeMap<usize, Box<Screen>>,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,
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
            screens: screens,
            next_id: 0,
            handles: BTreeMap::new(),
        }
    }

    pub fn can_read(&self, id: usize) -> Option<usize> {
        if let Some(handle) = self.handles.get(&id) {
            if let HandleKind::Screen(screen_i) = handle.kind {
                if let Some(screen) = self.screens.get(&screen_i) {
                    match screen.can_read() {
                        Some(count) => return Some(count),
                        None => if handle.flags & O_NONBLOCK == O_NONBLOCK {
                            return Some(0);
                        } else {
                            return None;
                        }
                    }
                }
            }
        }

        Some(0)
    }
}

impl SchemeMut for DisplayScheme {
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if path == b"input" {
            if uid == 0 {
                let id = self.next_id;
                self.next_id += 1;

                self.handles.insert(id, Handle {
                    kind: HandleKind::Input,
                    flags: flags
                });

                Ok(id)
            } else {
                Err(Error::new(EACCES))
            }
        } else {
            let path_str = str::from_utf8(path).unwrap_or("").trim_matches('/');
            let mut parts = path_str.split('/');
            let screen_i = parts.next().unwrap_or("").parse::<usize>().unwrap_or(0);
            if self.screens.contains_key(&screen_i) {
                for cmd in parts {
                    if cmd == "activate" {
                        self.active = screen_i;
                    }
                }

                let id = self.next_id;
                self.next_id += 1;

                self.handles.insert(id, Handle {
                    kind: HandleKind::Screen(screen_i),
                    flags: flags
                });

                Ok(id)
            } else {
                Err(Error::new(ENOENT))
            }
        }
    }

    fn dup(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        if ! buf.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let handle = self.handles.get(&id).map(|handle| handle.clone()).ok_or(Error::new(EBADF))?;

        let new_id = self.next_id;
        self.next_id += 1;

        self.handles.insert(new_id, handle.clone());

        Ok(new_id)
    }

    fn fevent(&mut self, id: usize, flags: usize) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(screen_i) = handle.kind {
            if let Some(mut screen) = self.screens.get_mut(&screen_i) {
                return screen.event(flags).and(Ok(screen_i));
            }
        }

        Err(Error::new(EBADF))
    }

    fn fmap(&mut self, id: usize, offset: usize, size: usize) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(screen_i) = handle.kind {
            if let Some(screen) = self.screens.get(&screen_i) {
                return screen.map(offset, size);
            }
        }

        Err(Error::new(EBADF))
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path_str = match handle.kind {
            HandleKind::Input => {
                format!("display:input/{}/{}", self.width, self.height)
            },
            HandleKind::Screen(screen_i) => if let Some(screen) = self.screens.get(&screen_i) {
                format!("display:{}/{}/{}", screen_i, screen.width(), screen.height())
            } else {
                return Err(Error::new(EBADF));
            }
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
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(screen_i) = handle.kind {
            if let Some(mut screen) = self.screens.get_mut(&screen_i) {
                if screen_i == self.active {
                    screen.sync();
                }
                return Ok(0);
            }
        }

        Err(Error::new(EBADF))
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(screen_i) = handle.kind {
            if let Some(mut screen) = self.screens.get_mut(&screen_i) {
                return screen.read(buf);
            }
        }

        Err(Error::new(EBADF))
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        match handle.kind {
            HandleKind::Input => if buf.len() == 1 && buf[0] >= 0xF4 {
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
                        EventOption::Key(key_event) => match key_event.scancode {
                            f @ 0x3B ... 0x44 => { // F1 through F10
                                new_active_opt = Some((f - 0x3A) as usize);
                            },
                            0x57 => { // F11
                                new_active_opt = Some(11);
                            },
                            0x58 => { // F12
                                new_active_opt = Some(12);
                            },
                            _ => ()
                        },
                        EventOption::Resize(resize_event) => {
                            println!("Resizing to {}, {}", resize_event.width, resize_event.height);
                            for (screen_i, screen) in self.screens.iter_mut() {
                                screen.resize(resize_event.width as usize, resize_event.height as usize);
                                if *screen_i == self.active {
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
                            screen.input(event);
                        }
                    }
                }

                Ok(events.len() * mem::size_of::<Event>())
            },
            HandleKind::Screen(screen_i) => if let Some(mut screen) = self.screens.get_mut(&screen_i) {
                screen.write(buf, screen_i == self.active)
            } else {
                Err(Error::new(EBADF))
            }
        }
    }

    fn seek(&mut self, id: usize, pos: usize, whence: usize) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(screen_i) = handle.kind {
            if let Some(mut screen) = self.screens.get_mut(&screen_i) {
                return screen.seek(pos, whence);
            }
        }

        Err(Error::new(EBADF))
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
}
