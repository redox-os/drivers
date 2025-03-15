use std::io;

use graphics_ipc::v1::{Damage, V1GraphicsHandle};
use inputd::{ConsumerHandle, ConsumerHandleEvent};
use orbclient::Event;

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
        let input_handle = ConsumerHandle::new_vt()?;

        let map = match input_handle.open_display() {
            Ok(display) => {
                let display_handle = V1GraphicsHandle::from_file(display)?;
                Some(
                    display_fd_map(display_handle)
                        .unwrap_or_else(|e| panic!("failed to map display: {e}")),
                )
            }
            Err(err) => {
                println!("fbbootlogd: No display present yet: {err}");
                None
            }
        };

        Ok(Self { input_handle, map })
    }

    pub fn handle_input_events(&mut self) {
        let mut events = [Event::new(); 16];
        loop {
            match self
                .input_handle
                .read_events(&mut events)
                .expect("fbbootlogd: error while reading events")
            {
                ConsumerHandleEvent::Events(&[]) => break,
                ConsumerHandleEvent::Events(_) => {}
                ConsumerHandleEvent::Handoff => {
                    eprintln!("fbbootlogd: handoff requested");

                    let new_display_handle = match self.input_handle.open_display() {
                        Ok(display) => V1GraphicsHandle::from_file(display).unwrap(),
                        Err(err) => {
                            println!("fbbootlogd: No display present yet: {err}");
                            continue;
                        }
                    };

                    match display_fd_map(new_display_handle) {
                        Ok(ok) => {
                            self.map = Some(ok);

                            eprintln!("fbbootlogd: handoff finished");
                        }
                        Err(err) => {
                            eprintln!("fbbootlogd: failed to open display: {}", err);
                        }
                    }
                }
            }
        }
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        if let Some(map) = &self.map {
            map.display_handle.sync_rects(&sync_rects).unwrap();
        }
    }
}
