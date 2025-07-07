use graphics_ipc::v2::{Damage, V2GraphicsHandle};
use inputd::ConsumerHandle;
use std::io;

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

        eprintln!("fbcond: Opened new display");

        let (width, height) = new_display_handle.display_size(0).unwrap();
        let fb = new_display_handle
            .create_dumb_framebuffer(width, height)
            .unwrap();

        match new_display_handle.map_dumb_framebuffer(fb, width, height) {
            Ok(map) => {
                eprintln!(
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
                eprintln!("failed to resize display: {}", err);
            }
        }
    }

    pub fn sync_rect(&mut self, damage: Damage) {
        if let Some(map) = &self.map {
            map.display_handle.update_plane(0, map.fb, damage).unwrap();
        }
    }
}
