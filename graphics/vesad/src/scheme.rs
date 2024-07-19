use std::collections::BTreeMap;
use std::str;

use syscall::{Error, EventFlags, MapFlags, Result, SchemeMut, EBADF, EINVAL, ENOENT, O_NONBLOCK};

use crate::{framebuffer::FrameBuffer, screen::GraphicScreen};

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
    pub notified_read: bool,
}

pub struct DisplayScheme {
    framebuffers: Vec<FrameBuffer>,
    active: VtIndex,
    pub vts: BTreeMap<VtIndex, BTreeMap<ScreenIndex, GraphicScreen>>,
    next_id: usize,
    pub handles: BTreeMap<usize, Handle>,
    pub inputd_handle: inputd::Handle,
}

impl DisplayScheme {
    pub fn new(framebuffers: Vec<FrameBuffer>, spec: &[()]) -> DisplayScheme {
        let mut inputd_handle = inputd::Handle::new("vesa").unwrap();

        let mut vts = BTreeMap::<VtIndex, BTreeMap<ScreenIndex, GraphicScreen>>::new();

        for &() in spec.iter() {
            let mut screens = BTreeMap::<ScreenIndex, GraphicScreen>::new();
            for fb_i in 0..framebuffers.len() {
                let fb = &framebuffers[fb_i];
                screens.insert(ScreenIndex(fb_i), GraphicScreen::new(fb.width, fb.height));
            }
            vts.insert(VtIndex(inputd_handle.register().unwrap()), screens);
        }

        DisplayScheme {
            framebuffers,
            active: VtIndex(1),
            vts,
            next_id: 0,
            handles: BTreeMap::new(),
            inputd_handle,
        }
    }

    pub fn can_read(&self, id: usize) -> Option<usize> {
        if let Some(handle) = self.handles.get(&id) {
            if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
                if let Some(screens) = self.vts.get(&vt_i) {
                    if let Some(screen) = screens.get(&screen_i) {
                        screen
                            .can_read()
                            .or(if handle.flags & O_NONBLOCK == O_NONBLOCK {
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
        println!(
            "Resizing framebuffer {} to {}, {} stride {}",
            fb_i, width, height, stride
        );

        unsafe {
            self.framebuffers[fb_i].resize(width, height, stride);
        }

        // Resize screens
        for (vt_i, screens) in self.vts.iter_mut() {
            for (screen_i, screen) in screens.iter_mut() {
                if screen_i.0 == fb_i {
                    screen.resize(width, height);
                    if *vt_i == self.active {
                        screen.redraw(&mut self.framebuffers[screen_i.0]);
                    }
                }
            }
        }
    }
}

impl SchemeMut for DisplayScheme {
    fn open(&mut self, path_str: &str, flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        if path_str == "handle" {
            let id = self.next_id;
            self.next_id += 1;
            self.handles.insert(
                id,
                Handle {
                    kind: HandleKind::Input,
                    flags,
                    events: EventFlags::empty(),
                    notified_read: false,
                },
            );
            return Ok(id);
        }

        let mut parts = path_str.split('/');
        let mut vt_screen = parts.next().unwrap_or("").split('.');
        let vt_i = VtIndex(vt_screen.next().unwrap_or("").parse::<usize>().unwrap_or(1));
        let screen_i = ScreenIndex(vt_screen.next().unwrap_or("").parse::<usize>().unwrap_or(0));
        if let Some(screens) = self.vts.get_mut(&vt_i) {
            if screens.get_mut(&screen_i).is_some() {
                let id = self.next_id;
                self.next_id += 1;

                self.handles.insert(
                    id,
                    Handle {
                        kind: HandleKind::Screen(vt_i, screen_i),
                        flags,
                        events: EventFlags::empty(),
                        notified_read: false,
                    },
                );

                Ok(id)
            } else {
                Err(Error::new(ENOENT))
            }
        } else {
            Err(Error::new(ENOENT))
        }
    }

    fn dup(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        if !buf.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let handle = self
            .handles
            .get(&id)
            .map(|handle| handle.clone())
            .ok_or(Error::new(EBADF))?;

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

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path_str = match handle.kind {
            HandleKind::Input => {
                //TODO: allow inputs associated with other framebuffers?
                format!(
                    "display:input/{}/{}",
                    self.framebuffers[0].width, self.framebuffers[0].height
                )
            }
            HandleKind::Screen(vt_i, screen_i) => {
                if let Some(screens) = self.vts.get(&vt_i) {
                    if let Some(screen) = screens.get(&screen_i) {
                        format!(
                            "display:{}.{}/{}/{}",
                            vt_i.0, screen_i.0, screen.width, screen.height
                        )
                    } else {
                        return Err(Error::new(EBADF));
                    }
                } else {
                    return Err(Error::new(EBADF));
                }
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
                        screen.sync(&mut self.framebuffers[screen_i.0]);
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
            HandleKind::Input => {
                use inputd::Cmd as DisplayCommand;

                let command = inputd::parse_command(buf).unwrap();

                match command {
                    DisplayCommand::Activate { vt } => {
                        let vt_i = VtIndex(vt);

                        if let Some(screens) = self.vts.get_mut(&vt_i) {
                            for (screen_i, screen) in screens.iter_mut() {
                                screen.redraw(&mut self.framebuffers[screen_i.0]);
                            }
                        }

                        self.active = vt_i;
                    }

                    DisplayCommand::Resize {
                        width,
                        height,
                        stride,
                        ..
                    } => {
                        self.resize(width as usize, height as usize, stride as usize);
                    }

                    // Nothing to do for deactivate :)
                    DisplayCommand::Deactivate(_) => {}
                }

                Ok(buf.len())
            }

            HandleKind::Screen(vt_i, screen_i) => {
                if let Some(screens) = self.vts.get_mut(&vt_i) {
                    if let Some(screen) = screens.get_mut(&screen_i) {
                        let count = screen.write(buf)?;
                        if vt_i == self.active {
                            screen.sync(&mut self.framebuffers[screen_i.0]);
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
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
    fn mmap_prep(&mut self, id: usize, off: u64, size: usize, _flags: MapFlags) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
            if let Some(screens) = self.vts.get(&vt_i) {
                if let Some(screen) = screens.get(&screen_i) {
                    if off as usize + size <= screen.offscreen.len() * 4 {
                        return Ok(screen.offscreen.as_ptr() as usize + off as usize);
                    } else {
                        return Err(Error::new(EINVAL));
                    }
                }
            }
        }

        Err(Error::new(EBADF))
    }
}
