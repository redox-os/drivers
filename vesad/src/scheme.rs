use std::collections::BTreeMap;
use std::{mem, ptr, slice, str};

use orbclient::{Event, EventOption};
use syscall::{Error, EventFlags, EACCES, EBADF, EINVAL, ENOENT, Map, O_NONBLOCK, Result, SchemeMut, PAGE_SIZE};

use crate::{
    display::Display,
    framebuffer::FrameBuffer,
    screen::{Screen, GraphicScreen, TextScreen},
};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug)]
pub struct VtIndex(usize);

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug)]
pub struct ScreenIndex(usize);

#[derive(Clone)]
pub enum HandleKind {
    Input,
    Screen(VtIndex, ScreenIndex),
}

#[derive(Clone)]
pub struct Handle {
    pub kind: HandleKind,
    pub flags: usize,
    pub events: EventFlags,
    pub notified_read: bool
}

pub struct DisplayScheme {
    framebuffers: Vec<FrameBuffer>,
    onscreens: Vec<&'static mut [u32]>,
    active: VtIndex,
    pub vts: BTreeMap<VtIndex, BTreeMap<ScreenIndex, Box<dyn Screen>>>,
    next_id: usize,
    pub handles: BTreeMap<usize, Handle>,
    super_key: bool,
    inputd_handle: inputd::Handle,
}

impl DisplayScheme {
    pub fn new(mut framebuffers: Vec<FrameBuffer>, spec: &[bool]) -> DisplayScheme {
        let mut inputd_handle  = inputd::Handle::new("vesa").unwrap();

        let mut onscreens = Vec::new();
        for fb in framebuffers.iter_mut() {
            onscreens.push(unsafe {
                fb.map().expect("vesad: failed to map framebuffer")
            });
        }

        let mut vts = BTreeMap::<VtIndex, BTreeMap<ScreenIndex, Box<dyn Screen>>>::new();

        for &vt_type in spec.iter() {
            let mut screens = BTreeMap::<ScreenIndex, Box<dyn Screen>>::new();
            for fb_i in 0..framebuffers.len() {
                let fb = &framebuffers[fb_i];
                screens.insert(ScreenIndex(fb_i), if vt_type {
                    Box::new(GraphicScreen::new(Display::new(fb.width, fb.height)))
                } else {
                    Box::new(TextScreen::new(Display::new(fb.width, fb.height)))
                });
            }
            vts.insert(VtIndex(inputd_handle.register().unwrap()), screens);
        }

        DisplayScheme {
            framebuffers,
            onscreens,
            active: VtIndex(1),
            vts,
            next_id: 0,
            handles: BTreeMap::new(),
            super_key: false,
            inputd_handle
        }
    }

    pub fn can_read(&self, id: usize) -> Option<usize> {
        if let Some(handle) = self.handles.get(&id) {
            if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
                if let Some(screens) = self.vts.get(&vt_i) {
                    if let Some(screen) = screens.get(&screen_i) {
                        screen.can_read().or(if handle.flags & O_NONBLOCK == O_NONBLOCK {
                            Some(0)
                        } else {
                            None
                        });
                    }
                }
            }
        }

        Some(0)
    }

    fn resize(&mut self, width: usize, height: usize, stride: usize) {
        //TODO: support resizing other outputs?
        let fb_i = 0;
        println!("Resizing framebuffer {} to {}, {} stride {}", fb_i, width, height, stride);

        // Unmap old onscreen
        unsafe {
            let slice = mem::take(&mut self.onscreens[fb_i]);
            syscall::funmap(slice.as_mut_ptr() as usize, (slice.len() * 4).next_multiple_of(PAGE_SIZE)).expect("vesad: failed to unmap framebuffer");
        }

        // Map new onscreen
        self.onscreens[fb_i] = unsafe {
            let size = stride * height;
            let onscreen_ptr = common::physmap(
                self.framebuffers[fb_i].phys,
                size * 4,
                common::Prot { read: true, write: true },
                common::MemoryType::WriteCombining,
            ).expect("vesad: failed to map framebuffer") as *mut u32;
            ptr::write_bytes(onscreen_ptr, 0, size);

            slice::from_raw_parts_mut(
                onscreen_ptr,
                size
            )
        };

        // Update size
        self.framebuffers[fb_i].width = width;
        self.framebuffers[fb_i].height = height;
        self.framebuffers[fb_i].stride = stride;

        // Resize screens
        for (vt_i, screens) in self.vts.iter_mut() {
            for (screen_i, screen) in screens.iter_mut() {
                if screen_i.0 == fb_i {
                    screen.resize(width, height);
                    if *vt_i == self.active {
                        screen.redraw(self.onscreens[fb_i], self.framebuffers[fb_i].stride);
                    }
                }
            }
        }
    }
}

impl SchemeMut for DisplayScheme {
    fn open(&mut self, path_str: &str, flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let mut parts = path_str.split('/');
        let mut vt_screen = parts.next().unwrap_or("").split('.');
        let vt_i = VtIndex(
            vt_screen.next().unwrap_or("").parse::<usize>().unwrap_or(1)
        );
        let screen_i = ScreenIndex(
            vt_screen.next().unwrap_or("").parse::<usize>().unwrap_or(0)
        );
        if let Some(screens) = self.vts.get_mut(&vt_i) {
            if let Some(screen) = screens.get_mut(&screen_i) {
                for cmd in parts {
                    if cmd == "activate" {
                        self.active = vt_i;
                        screen.redraw(
                            self.onscreens[screen_i.0],
                            self.framebuffers[screen_i.0].stride
                        );
                    }
                }

                let id = self.next_id;
                self.next_id += 1;

                self.handles.insert(id, Handle {
                    kind: HandleKind::Screen(vt_i, screen_i),
                    flags,
                    events: EventFlags::empty(),
                    notified_read: false
                });

                Ok(id)
            } else {
                Err(Error::new(ENOENT))
            }
        } else {
            Err(Error::new(ENOENT))
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

    fn fevent(&mut self, id: usize, flags: syscall::EventFlags) -> Result<syscall::EventFlags> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        handle.notified_read = false;

        if let HandleKind::Screen(_vt_i, _screen_i) = handle.kind {
            handle.events = flags;
            Ok(syscall::EventFlags::empty())
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fmap(&mut self, id: usize, map: &Map) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
            if let Some(screens) = self.vts.get(&vt_i) {
                if let Some(screen) = screens.get(&screen_i) {
                    return screen.map(map.offset, map.size);
                }
            }
        }

        Err(Error::new(EBADF))
    }
    fn fmap_old(&mut self, id: usize, map: &syscall::OldMap) -> syscall::Result<usize> {
        self.fmap(id, &Map {
            offset: map.offset,
            size: map.size,
            flags: map.flags,
            address: 0,
        })
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path_str = match handle.kind {
            HandleKind::Input => {
                //TODO: allow inputs associated with other framebuffers?
                format!("display:input/{}/{}", self.framebuffers[0].width, self.framebuffers[0].height)
            },
            HandleKind::Screen(vt_i, screen_i) => if let Some(screens) = self.vts.get(&vt_i) {
                if let Some(screen) = screens.get(&screen_i) {
                    format!("display:{}.{}/{}/{}", vt_i.0, screen_i.0, screen.width(), screen.height())
                } else {
                    return Err(Error::new(EBADF));
                }
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

        if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
            if let Some(screens) = self.vts.get_mut(&vt_i) {
                if let Some(screen) = screens.get_mut(&screen_i) {
                    if vt_i == self.active {
                        screen.sync(
                            self.onscreens[screen_i.0],
                            self.framebuffers[screen_i.0].stride
                        );
                    }
                    return Ok(0);
                }
            }
        }

        Err(Error::new(EBADF))
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
            if let Some(screens) = self.vts.get_mut(&vt_i) {
                if let Some(screen) = screens.get_mut(&screen_i) {
                    return screen.read(buf);
                }
            }
        }

        Err(Error::new(EBADF))
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        match handle.kind {
            HandleKind::Input => if buf.len() == 1 && buf[0] >= 0xF4 {
                let new_active = VtIndex((buf[0] - 0xF4) as usize + 1);
                if let Some(screens) = self.vts.get_mut(&new_active) {
                    self.active = new_active;
                    for (screen_i, screen) in screens.iter_mut() {
                        screen.redraw(
                            self.onscreens[screen_i.0],
                            self.framebuffers[screen_i.0].stride
                        );
                    }
                }
                Ok(1)
            } else {
                let events = unsafe { slice::from_raw_parts(buf.as_ptr() as *const Event, buf.len()/mem::size_of::<Event>()) };

                for event in events.iter() {
                    let mut new_active_opt = None;
                    match event.to_option() {
                        EventOption::Key(key_event) => match key_event.scancode {
                            f @ 0x3B ..= 0x44 if self.super_key => { // F1 through F10
                                new_active_opt = Some(VtIndex((f - 0x3A) as usize));
                            },
                            0x57 if self.super_key => { // F11
                                new_active_opt = Some(VtIndex(11));
                            },
                            0x58 if self.super_key => { // F12
                                new_active_opt = Some(VtIndex(12));
                            },
                            0x5B => { // Super
                                self.super_key = key_event.pressed;
                            },
                            _ => ()
                        },
                        EventOption::Resize(resize_event) => {
                            let width = resize_event.width as usize;
                            let height = resize_event.height as usize;
                            let stride = width; //TODO: get stride somehow
                            self.resize(width, height, stride);
                        },
                        _ => ()
                    };

                    if let Some(new_active) = new_active_opt {
                        if let Some(screens) = self.vts.get_mut(&new_active) {
                            self.active = new_active;
                            for (screen_i, screen) in screens.iter_mut() {
                                screen.redraw(
                                    self.onscreens[screen_i.0],
                                    self.framebuffers[screen_i.0].stride
                                );
                            }
                        }
                    } else {
                        if let Some(screens) = self.vts.get_mut(&self.active) {
                            //TODO: send input to other screens? Don't want extra events
                            if let Some(screen) = screens.get_mut(&ScreenIndex(0)) {
                                screen.input(event);
                            }
                        }
                    }
                }

                Ok(events.len() * mem::size_of::<Event>())
            },
            HandleKind::Screen(vt_i, screen_i) => if let Some(screens) = self.vts.get_mut(&vt_i) {
                if let Some(screen) = screens.get_mut(&screen_i) {
                    let count = screen.write(buf)?;
                    if vt_i == self.active {
                        screen.sync(
                            self.onscreens[screen_i.0],
                            self.framebuffers[screen_i.0].stride
                        );
                    }
                    Ok(count)
                } else {
                    Err(Error::new(EBADF))
                }
            } else {
                Err(Error::new(EBADF))
            }
        }
    }

    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
            if let Some(screens) = self.vts.get_mut(&vt_i) {
                if let Some(screen) = screens.get_mut(&screen_i) {
                    return screen.seek(pos, whence).map(|pos| pos as isize);
                }
            }
        }

        Err(Error::new(EBADF))
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
}
