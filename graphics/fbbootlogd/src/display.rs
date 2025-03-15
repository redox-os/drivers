use std::io;

use graphics_ipc::v1::{Damage, V1GraphicsHandle};
use inputd::ConsumerHandle;

fn display_fd_map(display_handle: V1GraphicsHandle) -> io::Result<DisplayMap> {
    let display_map = display_handle.map_display()?;
    Ok(DisplayMap {
        display_handle,
        inner: display_map,
    })
}

pub struct DisplayMap {
    display_handle: V1GraphicsHandle,
    pub inner: graphics_ipc::v1::DisplayMap,
}

pub struct Display {
    pub input_handle: ConsumerHandle,
    pub map: Option<DisplayMap>,
}

impl Display {
    pub fn open_first_vt() -> io::Result<Self> {
        let mut display = Self {
            input_handle: ConsumerHandle::new_vt()?,
            map: None,
        };

        display.handle_handoff();

        Ok(display)
    }

    pub fn handle_handoff(&mut self) {
        let new_display_handle = match self.input_handle.open_display() {
            Ok(display) => V1GraphicsHandle::from_file(display).unwrap(),
            Err(err) => {
                eprintln!("fbbootlogd: No display present yet: {err}");
                return;
            }
        };

        match display_fd_map(new_display_handle) {
            Ok(ok) => {
                self.map = Some(ok);

                eprintln!("fbbootlogd: mapped display");
            }
            Err(err) => {
                eprintln!("fbbootlogd: failed to open display: {}", err);
            }
        }
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        if let Some(map) = &self.map {
            map.display_handle.sync_rects(&sync_rects).unwrap();
        }
    }
}
