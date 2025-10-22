use std::collections::VecDeque;
use std::{cmp, ptr};

use console_draw::TextScreen;
use graphics_ipc::v2::V2GraphicsHandle;
use inputd::ConsumerHandle;
use orbclient::{Event, EventOption};
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
    text_buffer: console_draw::TextBuffer,
    is_scrollback: bool,
    scrollback_offset: usize,
    shift: bool,
}

impl FbbootlogScheme {
    pub fn new() -> FbbootlogScheme {
        let mut scheme = FbbootlogScheme {
            input_handle: ConsumerHandle::new_vt().expect("fbbootlogd: Failed to open vt"),
            display_map: None,
            text_screen: console_draw::TextScreen::new(),
            text_buffer: console_draw::TextBuffer::new(1000),
            is_scrollback: false,
            scrollback_offset: 1000,
            shift: false,
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

    pub fn handle_input(&mut self, ev: &Event) {
        match ev.to_option() {
            EventOption::Key(key_event) => {
                if key_event.scancode == 0x2A || key_event.scancode == 0x36 {
                    self.shift = key_event.pressed;
                } else if !key_event.pressed || !self.shift {
                    return;
                }
                match key_event.scancode {
                    0x48 => {
                        // Up
                        if self.scrollback_offset >= 1 {
                            self.scrollback_offset -= 1;
                        }
                    }
                    0x49 => {
                        // Page up
                        if self.scrollback_offset >= 10 {
                            self.scrollback_offset -= 10;
                        } else {
                            self.scrollback_offset = 0;
                        }
                    }
                    0x50 => {
                        // Down
                        self.scrollback_offset += 1;
                    }
                    0x51 => {
                        // Page down
                        self.scrollback_offset += 10;
                    }
                    0x47 => {
                        // Home
                        self.scrollback_offset = 0;
                    }
                    0x4F => {
                        // End
                        self.scrollback_offset = self.text_buffer.lines_max;
                    }
                    _ => return,
                }
            }
            _ => return,
        }
        self.handle_scrollback_render();
    }

    fn handle_scrollback_render(&mut self) {
        let Some(map) = &mut self.display_map else {
            return;
        };
        let buffer_len = self.text_buffer.lines.len();
        let dmap = &mut console_draw::DisplayMap {
            offscreen: map.inner.ptr_mut(),
            width: map.inner.width(),
            height: map.inner.height(),
        };
        // for both extra space on wrapping text and a scrollback indicator
        let spare_lines = 3;
        self.is_scrollback = true;
        self.scrollback_offset = cmp::min(
            self.scrollback_offset,
            buffer_len - dmap.height / 16 + spare_lines,
        );
        let mut i = self.scrollback_offset;
        self.text_screen
            .write(dmap, b"\x1B[1;1H\x1B[2J", &mut VecDeque::new());
        while i < buffer_len {
            let mut damage =
                self.text_screen
                    .write(dmap, &self.text_buffer.lines[i][..], &mut VecDeque::new());
            i += 1;
            let yd = (damage.y + damage.height) as usize;
            if i == buffer_len || yd + spare_lines * 16 > dmap.height {
                // render until end of screen
                damage.height = (dmap.height as u32) - damage.y;
                map.display_handle.update_plane(0, map.fb, damage).unwrap();
                self.is_scrollback = i < buffer_len;
                break;
            } else {
                map.display_handle.update_plane(0, map.fb, damage).unwrap();
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

                        let _ = map.display_handle.destroy_dumb_framebuffer(map.fb);

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
            self.text_buffer.write(buf);

            if !self.is_scrollback {
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
        }

        Ok(buf.len())
    }
}

impl FbbootlogScheme {
    pub fn on_close(&mut self, _id: usize) {}
}
