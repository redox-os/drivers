use std::collections::BTreeMap;
use std::os::fd::AsRawFd;

use event::{EventQueue, UserData};
use syscall::{Error, EventFlags, Result, SchemeMut, EBADF, EINVAL, ENOENT, O_NONBLOCK};

use crate::display::Display;
use crate::text::TextScreen;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug)]
pub struct VtIndex(usize);

impl VtIndex {
    pub const SCHEMA_SENTINEL: VtIndex = VtIndex(usize::MAX);
}

impl UserData for VtIndex {
    fn into_user_data(self) -> usize {
        self.0
    }

    fn from_user_data(user_data: usize) -> Self {
        VtIndex(user_data)
    }
}

#[derive(Clone)]
pub struct Handle {
    pub vt_i: VtIndex,
    pub flags: usize,
    pub events: EventFlags,
    pub notified_read: bool,
}

pub struct FbconScheme {
    pub vts: BTreeMap<VtIndex, TextScreen>,
    next_id: usize,
    pub handles: BTreeMap<usize, Handle>,
    pub inputd_handle: inputd::Handle,
}

impl FbconScheme {
    pub fn new(vt_ids: &[usize], event_queue: &mut EventQueue<VtIndex>) -> FbconScheme {
        let inputd_handle = inputd::Handle::new("vesa").unwrap();

        let mut vts = BTreeMap::new();

        for &vt_i in vt_ids {
            let display = Display::open_vt(vt_i).expect("Failed to open display for vt");
            event_queue
                .subscribe(
                    display.input_handle.as_raw_fd().as_raw_fd() as usize,
                    VtIndex(vt_i),
                    event::EventFlags::READ,
                )
                .expect("Failed to subscribe to input events for vt");
            vts.insert(VtIndex(vt_i), TextScreen::new(display));
        }

        FbconScheme {
            vts,
            next_id: 0,
            handles: BTreeMap::new(),
            inputd_handle,
        }
    }

    pub fn can_read(&self, id: usize) -> Option<usize> {
        if let Some(handle) = self.handles.get(&id) {
            if let Some(console) = self.vts.get(&handle.vt_i) {
                console
                    .can_read()
                    .or(if handle.flags & O_NONBLOCK == O_NONBLOCK {
                        Some(0)
                    } else {
                        None
                    });
            }
        }

        Some(0)
    }

    fn resize(&mut self, width: usize, height: usize, stride: usize) {
        for console in self.vts.values_mut() {
            console.resize(width, height);
        }
    }
}

impl SchemeMut for FbconScheme {
    fn open(&mut self, path_str: &str, flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let vt_i = VtIndex(path_str.parse::<usize>().map_err(|_| Error::new(ENOENT))?);
        if let Some(_console) = self.vts.get_mut(&vt_i) {
            let id = self.next_id;
            self.next_id += 1;

            self.handles.insert(
                id,
                Handle {
                    vt_i,
                    flags,
                    events: EventFlags::empty(),
                    notified_read: false,
                },
            );

            Ok(id)
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

        self.handles.insert(new_id, handle);

        Ok(new_id)
    }

    fn fevent(&mut self, id: usize, flags: syscall::EventFlags) -> Result<syscall::EventFlags> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        handle.notified_read = false;

        handle.events = flags;
        Ok(syscall::EventFlags::empty())
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path_str = format!("fbcon:{}", handle.vt_i.0);

        let path = path_str.as_bytes();

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(i)
    }

    fn fsync(&mut self, id: usize) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        return Ok(0);
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screen) = self.vts.get_mut(&handle.vt_i) {
            return screen.read(buf);
        }

        Err(Error::new(EBADF))
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(console) = self.vts.get_mut(&handle.vt_i) {
            console.write(buf)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
}
