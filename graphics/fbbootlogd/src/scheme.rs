use std::collections::VecDeque;

use graphics_ipc::v1::V1GraphicsHandle;
use inputd::ConsumerHandle;
use redox_scheme::Scheme;
use syscall::{Error, Result, EINVAL, ENOENT};

pub struct DisplayMap {
    display_handle: V1GraphicsHandle,
    inner: graphics_ipc::v1::DisplayMap,
}

pub struct FbbootlogScheme {
    pub input_handle: ConsumerHandle,
    display_map: Option<DisplayMap>,
    text_screen: console_draw::TextScreen,
}

impl FbbootlogScheme {
    pub fn new() -> FbbootlogScheme {
        let mut scheme = FbbootlogScheme {
            input_handle: ConsumerHandle::new_vt().expect("fbbootlogd: Failed to open vt"),
            display_map: None,
            text_screen: console_draw::TextScreen::new(),
        };

        scheme.handle_handoff();

        scheme
    }

    pub fn handle_handoff(&mut self) {
        let new_display_handle = match self.input_handle.open_display() {
            Ok(display) => V1GraphicsHandle::from_file(display).unwrap(),
            Err(err) => {
                eprintln!("fbbootlogd: No display present yet: {err}");
                return;
            }
        };

        match new_display_handle.map_display() {
            Ok(display_map) => {
                self.display_map = Some(DisplayMap {
                    display_handle: new_display_handle,
                    inner: display_map,
                });

                eprintln!("fbbootlogd: mapped display");
            }
            Err(err) => {
                eprintln!("fbbootlogd: failed to open display: {}", err);
            }
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
        if let Some(map) = &mut self.display_map {
            let damage = self.text_screen.write(
                &mut console_draw::DisplayMap {
                    offscreen: map.inner.ptr_mut(),
                    width: map.inner.width(),
                    height: map.inner.height(),
                },
                buf,
                &mut VecDeque::new(),
            );

            if let Some(map) = &self.display_map {
                map.display_handle.sync_rect(damage).unwrap();
            }
        }

        Ok(buf.len())
    }
}

impl FbbootlogScheme {
    pub fn on_close(&mut self, _id: usize) {}
}
