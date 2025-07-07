use graphics_ipc::v1::{Damage, V1GraphicsHandle};
use inputd::ConsumerHandle;
use std::io;

pub struct Display {
    pub input_handle: ConsumerHandle,
    pub map: Option<DisplayMap>,
}

pub struct DisplayMap {
    display_handle: V1GraphicsHandle,
    pub inner: graphics_ipc::v1::DisplayMap,
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
        let display_file = self.input_handle.open_display().unwrap();
        let new_display_handle = V1GraphicsHandle::from_file(display_file).unwrap();

        eprintln!("fbcond: Opened new display");

        match new_display_handle.map_display() {
            Ok(map) => {
                eprintln!(
                    "fbcond: Mapped new display with size {}x{}",
                    map.width(),
                    map.height()
                );

                self.map = Some(DisplayMap {
                    display_handle: new_display_handle,
                    inner: map,
                });
            }
            Err(err) => {
                eprintln!("failed to resize display: {}", err);
            }
        }
    }

    pub fn sync_rect(&mut self, sync_rect: Damage) {
        if let Some(map) = &self.map {
            map.display_handle.sync_rect(sync_rect).unwrap();
        }
    }
}
