use graphics_ipc::v1::{Damage, V1GraphicsHandle};
use inputd::ConsumerHandle;
use libredox::errno::ESTALE;
use orbclient::Event;
use std::mem;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use std::{io, slice};

fn read_to_slice<T: Copy>(
    file: BorrowedFd,
    buf: &mut [T],
) -> Result<usize, libredox::error::Error> {
    unsafe {
        libredox::call::read(
            file.as_raw_fd() as usize,
            slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * mem::size_of::<T>()),
        )
        .map(|count| count / mem::size_of::<T>())
    }
}

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
            match read_to_slice(self.input_handle.inner(), &mut events) {
                Err(err) if err.errno() == ESTALE => {
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

                Ok(0) => break,
                Ok(_count) => {}
                Err(err) => {
                    panic!("fbbootlogd: error while reading events: {err}");
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
