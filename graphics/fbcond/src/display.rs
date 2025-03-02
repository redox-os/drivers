use graphics_ipc::legacy::{Damage, LegacyGraphicsHandle};
use inputd::ConsumerHandle;
use std::io;

pub struct Display {
    pub input_handle: ConsumerHandle,
    pub map: Option<DisplayMap>,
}

pub struct DisplayMap {
    display_handle: LegacyGraphicsHandle,
    pub inner: graphics_ipc::legacy::DisplayMap,
}

impl Display {
    pub fn open_vt(vt: usize) -> io::Result<Self> {
        let input_handle = ConsumerHandle::new_vt()?;

        if let Ok(display_handle) = Self::open_display(&input_handle) {
            let map = display_handle
                .map_display()
                .unwrap_or_else(|e| panic!("failed to map display for VT #{vt}: {e}"));

            Ok(Self {
                input_handle,
                map: Some(DisplayMap {
                    display_handle,
                    inner: map,
                }),
            })
        } else {
            Ok(Self {
                input_handle,
                map: None,
            })
        }
    }

    /// Re-open the display after a handoff.
    pub fn reopen_for_handoff(&mut self) {
        eprintln!("fbcond: Performing handoff");

        let new_display_handle = Self::open_display(&self.input_handle).unwrap();

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

    fn open_display(input_handle: &ConsumerHandle) -> io::Result<LegacyGraphicsHandle> {
        let display_file = input_handle.open_display()?;

        LegacyGraphicsHandle::from_file(display_file)
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        if let Some(map) = &self.map {
            map.display_handle.sync_rects(&sync_rects).unwrap();
        }
    }
}
