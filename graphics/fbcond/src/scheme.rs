use std::collections::BTreeMap;
use std::os::fd::AsRawFd;

use event::{EventQueue, UserData};
use redox_scheme::SchemeBlock;
use syscall::{Error, EventFlags, Result, EBADF, ENOENT, O_NONBLOCK};

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
}

impl FbconScheme {
    pub fn new(vt_ids: &[usize], event_queue: &mut EventQueue<VtIndex>) -> FbconScheme {
        let mut vts = BTreeMap::new();

        for &vt_i in vt_ids {
            let display = Display::open_new_vt().expect("Failed to open display for vt");
            event_queue
                .subscribe(
                    display.input_handle.event_handle().as_raw_fd() as usize,
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
        }
    }
}

impl SchemeBlock for FbconScheme {
    fn open(
        &mut self,
        path_str: &str,
        flags: usize,
        _uid: u32,
        _gid: u32,
    ) -> Result<Option<usize>> {
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

            Ok(Some(id))
        } else {
            Err(Error::new(ENOENT))
        }
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

        let path_str = format!("fbcon:{}", handle.vt_i.0);

        let path = path_str.as_bytes();

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(Some(i))
    }

    fn fsync(&mut self, id: usize) -> Result<Option<usize>> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        return Ok(Some(0));
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if let Some(screen) = self.vts.get_mut(&handle.vt_i) {
            if !screen.can_read() && handle.flags & O_NONBLOCK != O_NONBLOCK {
                return Ok(None);
            } else {
                return screen.read(buf).map(Some);
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

        if let Some(console) = self.vts.get_mut(&handle.vt_i) {
            console.write(buf).map(Some)
        } else {
            Err(Error::new(EBADF))
        }
    }
}

impl FbconScheme {
    pub fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
