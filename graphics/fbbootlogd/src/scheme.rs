use std::collections::VecDeque;
use std::ptr;

use console_draw::TextScreen;
use graphics_ipc::v2::V2GraphicsHandle;
use inputd::ConsumerHandle;
use redox_scheme::scheme::SchemeSync;
use redox_scheme::{CallerCtx, OpenResult};
use syscall::schemev2::NewFdFlags;
use syscall::{Error, Result, EINVAL, ENOENT};

pub struct DisplayMap {
    display_handle: V2GraphicsHandle,
    fb: usize,
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
        let new_display_handle = match self.input_handle.open_display_v2() {
            Ok(display) => V2GraphicsHandle::from_file(display).unwrap(),
            Err(err) => {
                eprintln!("fbbootlogd: No display present yet: {err}");
                return;
            }
        };

        let (width, height) = new_display_handle.display_size(0).unwrap();
        let fb = new_display_handle
            .create_dumb_framebuffer(width, height)
            .unwrap();

        match new_display_handle.map_dumb_framebuffer(fb, width, height) {
            Ok(display_map) => {
                self.display_map = Some(DisplayMap {
                    display_handle: new_display_handle,
                    fb,
                    inner: display_map,
                });

                eprintln!("fbbootlogd: mapped display");
            }
            Err(err) => {
                eprintln!("fbbootlogd: failed to open display: {}", err);
            }
        }
    }

    fn handle_resize(map: &mut DisplayMap, text_screen: &mut TextScreen) {
        let (width, height) = match map.display_handle.display_size(0) {
            Ok((width, height)) => (width, height),
            Err(err) => {
                eprintln!("fbbootlogd: failed to get display size: {}", err);
                (map.inner.width() as u32, map.inner.height() as u32)
            }
        };

        if width as usize != map.inner.width() || height as usize != map.inner.height() {
            match map.display_handle.create_dumb_framebuffer(width, height) {
                Ok(fb) => match map.display_handle.map_dumb_framebuffer(fb, width, height) {
                    Ok(mut new_map) => {
                        let count = new_map.ptr().len();
                        unsafe {
                            ptr::write_bytes(new_map.ptr_mut() as *mut u32, 0, count);
                        }

                        text_screen.resize(
                            &mut console_draw::DisplayMap {
                                offscreen: map.inner.ptr_mut(),
                                width: map.inner.width(),
                                height: map.inner.height(),
                            },
                            &mut console_draw::DisplayMap {
                                offscreen: new_map.ptr_mut(),
                                width: new_map.width(),
                                height: new_map.height(),
                            },
                        );

                        map.fb = fb;
                        map.inner = new_map;

                        eprintln!("fbbootlogd: mapped display");
                    }
                    Err(err) => {
                        eprintln!("fbbootlogd: failed to open display: {}", err);
                    }
                },
                Err(err) => {
                    eprintln!("fbbootlogd: failed to create framebuffer: {}", err);
                }
            }
        }
    }
}

impl SchemeSync for FbbootlogScheme {
    fn open(&mut self, path_str: &str, _flags: usize, _ctx: &CallerCtx) -> Result<OpenResult> {
        if !path_str.is_empty() {
            return Err(Error::new(ENOENT));
        }

        Ok(OpenResult::ThisScheme {
            number: 0,
            flags: NewFdFlags::empty(),
        })
    }

    fn fpath(&mut self, _id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        let path = b"fbbootlog:";

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(i)
    }

    fn fsync(&mut self, _id: usize, _ctx: &CallerCtx) -> Result<()> {
        Ok(())
    }

    fn read(
        &mut self,
        _id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        Err(Error::new(EINVAL))
    }

    fn write(
        &mut self,
        _id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        if let Some(map) = &mut self.display_map {
            Self::handle_resize(map, &mut self.text_screen);

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
                map.display_handle.update_plane(0, map.fb, damage).unwrap();
            }
        }

        Ok(buf.len())
    }
}

impl FbbootlogScheme {
    pub fn on_close(&mut self, _id: usize) {}
}
