use console_draw::TextScreen;
use graphics_ipc::v2::{Damage, V2GraphicsHandle};
use inputd::ConsumerHandle;
use std::{io, ptr};

pub struct Display {
    pub input_handle: ConsumerHandle,
    pub map: Option<DisplayMap>,
}

pub struct DisplayMap {
    display_handle: V2GraphicsHandle,
    fb: usize,
    pub inner: graphics_ipc::v2::DisplayMap,
}

impl Display {
    pub fn open_new_vt() -> io::Result<Self> {
        let mut display = Self {
            input_handle: ConsumerHandle::new_vt()?,
            map: None,
        };

        display.reopen_for_handoff();

        Ok(display)
    }

    /// Re-open the display after a handoff.
    pub fn reopen_for_handoff(&mut self) {
        let display_file = self.input_handle.open_display_v2().unwrap();
        let new_display_handle = V2GraphicsHandle::from_file(display_file).unwrap();

        log::debug!("fbcond: Opened new display");

        let (width, height) = new_display_handle.display_size(0).unwrap();
        let fb = new_display_handle
            .create_dumb_framebuffer(width, height)
            .unwrap();

        match new_display_handle.map_dumb_framebuffer(fb, width, height) {
            Ok(map) => {
                log::debug!(
                    "fbcond: Mapped new display with size {}x{}",
                    map.width(),
                    map.height()
                );

                self.map = Some(DisplayMap {
                    display_handle: new_display_handle,
                    fb,
                    inner: map,
                });
            }
            Err(err) => {
                log::error!("failed to map display: {}", err);
            }
        }
    }

    pub fn handle_resize(map: &mut DisplayMap, text_screen: &mut TextScreen) {
        let (width, height) = match map.display_handle.display_size(0) {
            Ok((width, height)) => (width, height),
            Err(err) => {
                log::error!("fbcond: failed to get display size: {}", err);
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

                        log::debug!("fbcond: mapped display");
                    }
                    Err(err) => {
                        log::error!("fbcond: failed to open display: {}", err);
                    }
                },
                Err(err) => {
                    log::error!("fbcond: failed to create framebuffer: {}", err);
                }
            }
        }
    }

    pub fn sync_rect(&mut self, damage: Damage) {
        if let Some(map) = &self.map {
            map.display_handle.update_plane(0, map.fb, damage).unwrap();
        }
    }
}
