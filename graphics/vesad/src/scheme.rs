use std::collections::BTreeMap;
use std::str;

use inputd::{VtEvent, VtEventKind};
use redox_scheme::SchemeBlockMut;
use syscall::{Error, EventFlags, MapFlags, Result, EBADF, EINVAL, ENOENT, O_NONBLOCK};

use crate::{framebuffer::FrameBuffer, screen::GraphicScreen};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug)]
pub struct VtIndex(usize);

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug)]
pub struct ScreenIndex(usize);

#[derive(Clone)]
pub struct Handle {
    pub vt: VtIndex,
    pub screen: ScreenIndex,

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
    pub inputd_handle: inputd::DisplayHandle,
}

impl DisplayScheme {
    pub fn new(framebuffers: Vec<FrameBuffer>, spec: &[()]) -> DisplayScheme {
        let mut inputd_handle = inputd::DisplayHandle::new_early("vesa").unwrap();

        let mut vts = BTreeMap::<VtIndex, BTreeMap<ScreenIndex, GraphicScreen>>::new();

        for &() in spec.iter() {
            let mut screens = BTreeMap::<ScreenIndex, GraphicScreen>::new();
            for fb_i in 0..framebuffers.len() {
                let fb = &framebuffers[fb_i];
                screens.insert(ScreenIndex(fb_i), GraphicScreen::new(fb.width, fb.height));
            }
            vts.insert(VtIndex(inputd_handle.register_vt().unwrap()), screens);
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

    pub fn handle_vt_event(&mut self, vt_event: VtEvent) {
        match vt_event.kind {
            VtEventKind::Activate => {
                let vt_i = VtIndex(vt_event.vt);

                if let Some(screens) = self.vts.get_mut(&vt_i) {
                    for (screen_i, screen) in screens.iter_mut() {
                        screen.redraw(&mut self.framebuffers[screen_i.0]);
                    }
                }

                self.active = vt_i;
            }
            VtEventKind::Deactivate => {
                // Nothing to do for deactivate :)
            }
            VtEventKind::Resize => {
                self.resize(
                    vt_event.width as usize,
                    vt_event.height as usize,
                    vt_event.stride as usize,
                );
            }
        }
    }
}

impl SchemeBlockMut for DisplayScheme {
    fn open(
        &mut self,
        path_str: &str,
        flags: usize,
        _uid: u32,
        _gid: u32,
    ) -> Result<Option<usize>> {
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
                        vt: vt_i,
                        screen: screen_i,

                        flags,
                        events: EventFlags::empty(),
                        notified_read: false,
                    },
                );

                Ok(Some(id))
            } else {
                Err(Error::new(ENOENT))
            }
        } else {
            Err(Error::new(ENOENT))
        }
    }

    fn dup(&mut self, id: usize, buf: &[u8]) -> Result<Option<usize>> {
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

        Ok(Some(new_id))
    }

    fn fevent(
        &mut self,
        id: usize,
        flags: syscall::EventFlags,
    ) -> Result<Option<syscall::EventFlags>> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        handle.notified_read = false;
        handle.events = flags;
        Ok(Some(syscall::EventFlags::empty()))
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path_str = {
            if let Some(screens) = self.vts.get(&handle.vt) {
                if let Some(screen) = screens.get(&handle.screen) {
                    format!(
                        "display:{}.{}/{}/{}",
                        handle.vt.0, handle.screen.0, screen.width, screen.height
                    )
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

        Ok(Some(i))
    }

    fn fsync(&mut self, id: usize) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screens) = self.vts.get_mut(&handle.vt) {
            if let Some(screen) = screens.get_mut(&handle.screen) {
                if handle.vt == self.active {
                    screen.redraw(&mut self.framebuffers[handle.screen.0]);
                }
                return Ok(Some(0));
            }
        }

        Err(Error::new(EBADF))
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screens) = self.vts.get_mut(&handle.vt) {
            if let Some(screen) = screens.get_mut(&handle.screen) {
                let nread = screen.read(buf)?;
                if nread != 0 {
                    return Ok(Some(nread));
                } else {
                    if handle.flags & O_NONBLOCK == O_NONBLOCK {
                        return Ok(Some(0));
                    } else {
                        return Ok(None);
                    }
                }
            }
        }

        Err(Error::new(EBADF))
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screens) = self.vts.get_mut(&handle.vt) {
            if let Some(screen) = screens.get_mut(&handle.screen) {
                if handle.vt == self.active {
                    screen
                        .write(buf, Some(&mut self.framebuffers[handle.screen.0]))
                        .map(|count| Some(count))
                } else {
                    screen.write(buf, None).map(|count| Some(count))
                }
            } else {
                Err(Error::new(EBADF))
            }
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn close(&mut self, id: usize) -> Result<Option<usize>> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(Some(0))
    }
    fn mmap_prep(
        &mut self,
        id: usize,
        off: u64,
        size: usize,
        _flags: MapFlags,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screens) = self.vts.get(&handle.vt) {
            if let Some(screen) = screens.get(&handle.screen) {
                if off as usize + size <= screen.offscreen.len() * 4 {
                    return Ok(Some(screen.offscreen.as_ptr() as usize + off as usize));
                } else {
                    return Err(Error::new(EINVAL));
                }
            }
        }

        Err(Error::new(EBADF))
    }
}
