use inputd::{ConsumerHandle, Damage};
use libredox::flag;
use std::mem;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::{fs::File, io, os::unix::io::AsRawFd, slice};

fn display_fd_map(
    width: usize,
    height: usize,
    display_file: &mut File,
) -> syscall::Result<DisplayMap> {
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
    Resize { width: usize, height: usize },
    ReopenForHandoff,
    SyncRects(Vec<Damage>),
}

pub struct Display {
    pub input_handle: Arc<ConsumerHandle>,
    cmd_tx: Sender<DisplayCommand>,
    pub map: Arc<Mutex<DisplayMap>>,
}

impl Display {
    pub fn open_vt(vt: usize) -> io::Result<Self> {
        let input_handle = Arc::new(ConsumerHandle::for_vt(vt)?);

        let (mut display_file, width, height) = Self::open_display(&input_handle)?;

        let map = Arc::new(Mutex::new(
            display_fd_map(width, height, &mut display_file)
                .unwrap_or_else(|e| panic!("failed to map display for VT #{vt}: {e}")),
        ));

        let (cmd_tx, cmd_rx) = mpsc::channel();

        let input_handle_clone = input_handle.clone();
        let map_clone = map.clone();
        std::thread::spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    DisplayCommand::Resize { width, height } => {
                        match display_fd_map(width, height, &mut display_file) {
                            Ok(ok) => {
                                *map_clone.lock().unwrap() = ok;
                            }
                            Err(err) => {
                                eprintln!(
                                    "failed to resize display to {}x{}: {}",
                                    width, height, err
                                );
                            }
                        }
                    }
                    DisplayCommand::ReopenForHandoff => {
                        eprintln!("fbcond: Performing handoff");

                        let (mut new_display_file, width, height) =
                            Self::open_display(&input_handle_clone).unwrap();

                        eprintln!("fbcond: Opened new display for VT #{vt}");

                        match display_fd_map(width, height, &mut new_display_file) {
                            Ok(ok) => {
                                *map_clone.lock().unwrap() = ok;
                                display_file = new_display_file;

                                eprintln!("fbcond: Mapped new display for VT #{vt}");
                            }
                            Err(err) => {
                                eprintln!(
                                    "failed to resize display to {}x{}: {}",
                                    width, height, err
                                );
                            }
                        }
                    }
                    DisplayCommand::SyncRects(sync_rects) => unsafe {
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
        });

        Ok(Self {
            input_handle,
            cmd_tx,
            map,
        })
    }

    /// Re-open the display after a handoff.
    ///
    /// Once re-opening is finished, you must call [`resize`] to map the new framebuffer.
    ///
    /// Warning: This must be called in a background thread to avoid a deadlock when the
    /// graphics driver (indirectly) writes logs to fbcond.
    pub fn reopen_for_handoff(&mut self) {
        self.cmd_tx.send(DisplayCommand::ReopenForHandoff).unwrap();
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

    pub fn resize(&mut self, width: usize, height: usize) {
        self.cmd_tx
            .send(DisplayCommand::Resize { width, height })
            .unwrap();
    }

    pub fn sync_rects(&mut self, sync_rects: Vec<Damage>) {
        self.cmd_tx
            .send(DisplayCommand::SyncRects(sync_rects))
            .unwrap();
    }
}
