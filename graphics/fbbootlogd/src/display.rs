use event::{user_data, EventQueue};
use graphics_ipc::v1::{Damage, V1GraphicsHandle};
use inputd::ConsumerHandle;
use libredox::errno::ESTALE;
use orbclient::Event;
use std::mem;
use std::os::fd::BorrowedFd;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::{io, os::unix::io::AsRawFd, slice};

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
        display_handle: Arc::new(display_handle),
        inner: display_map,
    })
}

pub struct DisplayMap {
    display_handle: Arc<V1GraphicsHandle>,
    pub inner: graphics_ipc::v1::DisplayMap,
}

enum DisplayCommand {
    SyncRects(Vec<Damage>),
}

pub struct Display {
    cmd_tx: Sender<DisplayCommand>,
    pub map: Arc<Mutex<Option<DisplayMap>>>,
}

impl Display {
    pub fn open_first_vt() -> io::Result<Self> {
        let input_handle = ConsumerHandle::new_vt()?;

        let map = match input_handle.open_display() {
            Ok(display) => {
                let display_handle = V1GraphicsHandle::from_file(display)?;
                Arc::new(Mutex::new(Some(
                    display_fd_map(display_handle)
                        .unwrap_or_else(|e| panic!("failed to map display: {e}")),
                )))
            }
            Err(err) => {
                println!("fbbootlogd: No display present yet: {err}");
                Arc::new(Mutex::new(None))
            }
        };

        let map_clone = map.clone();
        std::thread::spawn(move || {
            Self::handle_input_events(map_clone, input_handle);
        });

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let map_clone = map.clone();
        std::thread::spawn(move || {
            Self::handle_sync_rect(map_clone, cmd_rx);
        });

        Ok(Self { cmd_tx, map })
    }

    fn handle_input_events(map: Arc<Mutex<Option<DisplayMap>>>, input_handle: ConsumerHandle) {
        let event_queue = EventQueue::new().expect("fbbootlogd: failed to create event queue");

        user_data! {
            enum Source {
                Input,
            }
        }

        // FIXME listen for resize events from inputd and handle them

        event_queue
            .subscribe(
                input_handle.inner().as_raw_fd() as usize,
                Source::Input,
                event::EventFlags::READ,
            )
            .expect("fbbootlogd: failed to subscribe to scheme events");

        let mut events = [Event::new(); 16];
        for Source::Input in event_queue.map(|event| event.unwrap().user_data) {
            match read_to_slice(input_handle.inner(), &mut events) {
                Err(err) if err.errno() == ESTALE => {
                    eprintln!("fbbootlogd: handoff requested");

                    let new_display_handle = match input_handle.open_display() {
                        Ok(display) => V1GraphicsHandle::from_file(display).unwrap(),
                        Err(err) => {
                            println!("fbbootlogd: No display present yet: {err}");
                            continue;
                        }
                    };

                    match display_fd_map(new_display_handle) {
                        Ok(ok) => {
                            *map.lock().unwrap() = Some(ok);

                            eprintln!("fbbootlogd: handoff finished");
                        }
                        Err(err) => {
                            eprintln!("fbbootlogd: failed to open display: {}", err);
                        }
                    }
                }

                Ok(_count) => {}
                Err(err) => {
                    panic!("fbbootlogd: error while reading events: {err}");
                }
            }
        }
    }

    fn handle_sync_rect(map: Arc<Mutex<Option<DisplayMap>>>, cmd_rx: Receiver<DisplayCommand>) {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                DisplayCommand::SyncRects(sync_rects) => {
                    // We may not hold this lock across the write call to avoid deadlocking if the
                    // graphics driver tries to write to the bootlog.
                    let display_handle = if let Some(map) = &*map.lock().unwrap() {
                        map.display_handle.clone()
                    } else {
                        continue;
                    };
                    display_handle.sync_rects(&sync_rects).unwrap();
                }
            }
        }
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        self.cmd_tx
            .send(DisplayCommand::SyncRects(sync_rects))
            .unwrap();
    }
}
