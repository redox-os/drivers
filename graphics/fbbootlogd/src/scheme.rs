use std::collections::{BTreeMap, VecDeque};

use redox_scheme::Scheme;
use syscall::{Error, EventFlags, Result, EBADF, EINVAL, ENOENT};

use crate::display::Display;

pub struct Handle {
    pub events: EventFlags,
    pub notified_read: bool,
}

pub struct FbbootlogScheme {
    display: Display,
    text_screen: console_draw::TextScreen,
    next_id: usize,
    pub handles: BTreeMap<usize, Handle>,
}

impl FbbootlogScheme {
    pub fn new() -> FbbootlogScheme {
        FbbootlogScheme {
            display: Display::open_first_vt().expect("Failed to open display for vt"),
            text_screen: console_draw::TextScreen::new(),
            next_id: 0,
            handles: BTreeMap::new(),
        }
    }
}

impl Scheme for FbbootlogScheme {
    fn open(&mut self, path_str: &str, _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        if !path_str.is_empty() {
            return Err(Error::new(ENOENT));
        }

        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(
            id,
            Handle {
                events: EventFlags::empty(),
                notified_read: false,
            },
        );

        Ok(id)
    }

    fn fevent(&mut self, id: usize, flags: syscall::EventFlags) -> Result<syscall::EventFlags> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        handle.notified_read = false;

        handle.events = flags;
        Ok(syscall::EventFlags::empty())
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let path = b"fbbootlog:";

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

    fn read(
        &mut self,
        id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        Err(Error::new(EINVAL))
    }

    fn write(&mut self, id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let mut map = self.display.map.lock().unwrap();
        let damage = self.text_screen.write(
            &mut console_draw::DisplayMap {
                offscreen: map.inner.ptr_mut(),
                width: map.inner.width(),
                height: map.inner.height(),
            },
            buf,
            &mut VecDeque::new(),
        );
        drop(map);

        self.display.sync_rects(damage);

        Ok(buf.len())
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
}
