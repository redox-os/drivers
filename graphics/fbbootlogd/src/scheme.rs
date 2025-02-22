use std::collections::VecDeque;

use redox_scheme::Scheme;
use syscall::{Error, Result, EINVAL, ENOENT};

use crate::display::Display;

pub struct FbbootlogScheme {
    display: Display,
    text_screen: console_draw::TextScreen,
}

impl FbbootlogScheme {
    pub fn new() -> FbbootlogScheme {
        FbbootlogScheme {
            display: Display::open_first_vt().expect("Failed to open display for vt"),
            text_screen: console_draw::TextScreen::new(),
        }
    }
}

impl Scheme for FbbootlogScheme {
    fn open(&mut self, path_str: &str, _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        if !path_str.is_empty() {
            return Err(Error::new(ENOENT));
        }

        Ok(0)
    }

    fn fpath(&mut self, _id: usize, buf: &mut [u8]) -> Result<usize> {
        let path = b"fbbootlog:";

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(i)
    }

    fn fsync(&mut self, _id: usize) -> Result<usize> {
        Ok(0)
    }

    fn read(
        &mut self,
        _id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        Err(Error::new(EINVAL))
    }

    fn write(&mut self, _id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
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

    fn close(&mut self, _id: usize) -> Result<usize> {
        Ok(0)
    }
}
