use event::{user_data, EventQueue};
use inputd::{ConsumerHandle, Damage};
use libredox::errno::ESTALE;
use libredox::flag;
use orbclient::Event;
use std::mem;
use std::os::fd::BorrowedFd;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::{fs::File, io, os::unix::io::AsRawFd, slice};

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

fn display_fd_map(width: usize, height: usize, display_file: File) -> syscall::Result<DisplayMap> {
    unsafe {
        let display_ptr = libredox::call::mmap(libredox::call::MmapArgs {
            fd: display_file.as_raw_fd() as usize,
            offset: 0,
            length: (width * height * 4),
            prot: flag::PROT_READ | flag::PROT_WRITE,
            flags: flag::MAP_SHARED,
            addr: core::ptr::null_mut(),
        })?;
        let display_slice = slice::from_raw_parts_mut(display_ptr as *mut u32, width * height);
        Ok(DisplayMap {
            display_file: Arc::new(display_file),
            offscreen: display_slice,
            width,
            height,
        })
    }
}

unsafe fn display_fd_unmap(image: *mut [u32]) {
    let _ = libredox::call::munmap(image as *mut (), image.len());
}

pub struct DisplayMap {
    display_file: Arc<File>,
    pub offscreen: *mut [u32],
    pub width: usize,
    pub height: usize,
}

unsafe impl Send for DisplayMap {}
unsafe impl Sync for DisplayMap {}

impl Drop for DisplayMap {
    fn drop(&mut self) {
        unsafe {
            display_fd_unmap(self.offscreen);
        }
    }
}

enum DisplayCommand {
    SyncRects(Vec<Damage>),
}

pub struct Display {
    cmd_tx: Sender<DisplayCommand>,
    pub map: Arc<Mutex<DisplayMap>>,
}

impl Display {
    pub fn open_first_vt() -> io::Result<Self> {
        let input_handle = ConsumerHandle::for_vt(1)?;

        let (display_file, width, height) = Self::open_display(&input_handle)?;

        let map = Arc::new(Mutex::new(
            display_fd_map(width, height, display_file)
                .unwrap_or_else(|e| panic!("failed to map display: {e}")),
        ));

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

    fn open_display(input_handle: &ConsumerHandle) -> io::Result<(File, usize, usize)> {
        let display_file = input_handle.open_display()?;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = libredox::call::fpath(display_file.as_raw_fd() as usize, &mut buf)
            .unwrap_or_else(|e| {
                panic!("Could not read display path with fpath(): {e}");
            });

        let url =
            String::from_utf8(Vec::from(&buf[..count])).expect("Could not create Utf8 Url String");
        let path = url.split(':').nth(1).expect("Could not get path from url");

        let mut path_parts = path.split('/').skip(1);
        let width = path_parts
            .next()
            .unwrap_or("")
            .parse::<usize>()
            .unwrap_or(0);
        let height = path_parts
            .next()
            .unwrap_or("")
            .parse::<usize>()
            .unwrap_or(0);

        Ok((display_file, width, height))
    }

    fn handle_input_events(map: Arc<Mutex<DisplayMap>>, input_handle: ConsumerHandle) {
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

                    let (new_display_file, width, height) =
                        Self::open_display(&input_handle).unwrap();

                    match display_fd_map(width, height, new_display_file) {
                        Ok(ok) => {
                            *map.lock().unwrap() = ok;

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

    fn handle_sync_rect(map: Arc<Mutex<DisplayMap>>, cmd_rx: Receiver<DisplayCommand>) {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                DisplayCommand::SyncRects(sync_rects) => unsafe {
                    // We may not hold this lock across the write call to avoid deadlocking if the
                    // graphics driver tries to write to the bootlog.
                    let display_file = map.lock().unwrap().display_file.clone();
                    libredox::call::write(
                        display_file.as_raw_fd() as usize,
                        slice::from_raw_parts(
                            sync_rects.as_ptr() as *const u8,
                            sync_rects.len() * mem::size_of::<Damage>(),
                        ),
                    )
                    .unwrap();
                },
            }
        }
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        self.cmd_tx
            .send(DisplayCommand::SyncRects(sync_rects))
            .unwrap();
    }
}
