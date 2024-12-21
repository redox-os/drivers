//! `:input`
//!
//! A seperate scheme is required since all of the input from different input devices is required
//! to be combined into a single stream which is later going to be processed by the "consumer"
//! which usually is Orbital.
//!
//! ## Input Device ("producer")
//! Write events to `input:producer`.
//!
//! ## Input Consumer ("consumer")
//! Read events from `input:consumer`. Optionally, set the `EVENT_READ` flag to be notified when
//! events are available.

use std::collections::BTreeMap;
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};

use inputd::{Cmd, VtActivate};

use redox_scheme::{RequestKind, SchemeMut, SignalBehavior, Socket, V2};

use orbclient::{Event, EventOption};
use syscall::{Error as SysError, EventFlags, EINVAL};

enum Handle {
    Producer,
    Consumer {
        events: EventFlags,
        pending: Vec<u8>,
        notified: bool,
        vt: usize,
    },
    Display {
        device: String,
    },
    Control,
}

impl Handle {
    pub fn is_producer(&self) -> bool {
        matches!(self, Handle::Producer)
    }
}

struct Vt {
    display: String,
    index: usize,

    /// This is *required* to be lazily initialized since opening the handle to the display
    /// requires the system call to return first. Otherwise, it will block indefinitely.
    handle_file: Option<File>,
}

impl Vt {
    fn new(display: impl Into<String>, index: usize) -> Self {
        Self {
            display: display.into(),
            handle_file: None,
            index,
        }
    }

    fn send_command(&mut self, cmd: Cmd) -> Result<(), libredox::error::Error> {
        let handle_file = self
            .handle_file
            .get_or_insert_with(|| File::open(format!("/scheme/{}/handle", self.display)).unwrap());
        inputd::send_comand(handle_file, cmd)
    }
}

struct InputScheme {
    handles: BTreeMap<usize, Handle>,

    next_id: AtomicUsize,
    next_vt_id: AtomicUsize,

    vts: BTreeMap<usize, Vt>,
    super_key: bool,
    active_vt: Option<usize>,

    pending_activate: Option<VtActivate>,
    has_new_events: bool,
}

impl InputScheme {
    fn new() -> Self {
        Self {
            next_id: AtomicUsize::new(0),
            next_vt_id: AtomicUsize::new(1),

            handles: BTreeMap::new(),
            vts: BTreeMap::new(),
            super_key: false,
            active_vt: None,

            pending_activate: None,
            has_new_events: false,
        }
    }
}

impl SchemeMut for InputScheme {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        let mut path_parts = path.split('/');

        let command = path_parts.next().ok_or(SysError::new(EINVAL))?;
        let fd = self.next_id.fetch_add(1, Ordering::SeqCst);

        let handle_ty = match command {
            "producer" => Handle::Producer,
            "consumer" => {
                let target = path_parts
                    .next()
                    .and_then(|x| x.parse::<usize>().ok())
                    .ok_or(SysError::new(EINVAL))?;

                Handle::Consumer {
                    events: EventFlags::empty(),
                    pending: Vec::new(),
                    notified: false,
                    vt: target,
                }
            }
            "handle" => {
                let display = path_parts.collect::<Vec<_>>().join(".");
                Handle::Display { device: display }
            }
            "control" => Handle::Control,

            _ => {
                log::error!("inputd: invalid path {path}");
                return Err(SysError::new(EINVAL));
            }
        };

        log::info!("inputd: {path} channel has been opened");

        self.handles.insert(fd, handle_ty);
        Ok(fd)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).ok_or(SysError::new(EINVAL))?;

        if let Handle::Consumer { vt, .. } = handle {
            let display = self.vts.get(vt).ok_or(SysError::new(EINVAL))?;
            let vt = format!("{}:{vt}", display.display);

            let size = core::cmp::min(vt.len(), buf.len());
            buf[..size].copy_from_slice(&vt.as_bytes()[..size]);

            Ok(size)
        } else {
            Err(SysError::new(EINVAL))
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        match handle {
            Handle::Consumer { pending, .. } => {
                let copy = core::cmp::min(pending.len(), buf.len());

                for (i, byte) in pending.drain(..copy).enumerate() {
                    buf[i] = byte;
                }

                Ok(copy)
            }

            Handle::Display { device } => {
                assert!(buf.is_empty());

                let vt = self.next_vt_id.fetch_add(1, Ordering::SeqCst);
                self.vts.insert(vt, Vt::new(device.clone(), vt));
                Ok(vt)
            }

            Handle::Producer => {
                log::error!("inputd: producer tried to read");
                return Err(SysError::new(EINVAL));
            }
            Handle::Control => {
                log::error!("inputd: control tried to read");
                return Err(SysError::new(EINVAL));
            }
        }
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        self.has_new_events = true;

        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        match handle {
            Handle::Control => {
                if buf.len() != core::mem::size_of::<VtActivate>() {
                    log::error!("inputd: control tried to write incorrectly sized command");
                    return Err(SysError::new(EINVAL));
                }

                // SAFETY: We have verified the size of the buffer above.
                let cmd = unsafe { &*buf.as_ptr().cast::<VtActivate>() };

                self.pending_activate = Some(cmd.clone());

                return Ok(buf.len());
            }

            Handle::Consumer { .. } => {
                log::error!("inputd: consumer tried to write");
                return Err(SysError::new(EINVAL));
            }
            Handle::Display { .. } => {
                log::error!("inputd: display tried to write");
                return Err(SysError::new(EINVAL));
            }
            Handle::Producer => {}
        }

        if buf.len() == 1 && buf[0] > 0xf4 {
            return Ok(1);
        }

        let events = unsafe {
            core::slice::from_raw_parts(
                buf.as_ptr() as *const Event,
                buf.len() / core::mem::size_of::<Event>(),
            )
        };

        'out: for event in events.iter() {
            let mut new_active_opt = None;
            match event.to_option() {
                EventOption::Key(key_event) => match key_event.scancode {
                    f @ 0x3B..=0x44 if self.super_key => {
                        // F1 through F10
                        new_active_opt = Some((f - 0x3A) as usize);
                    }

                    0x57 if self.super_key => {
                        // F11
                        new_active_opt = Some(11);
                    }

                    0x58 if self.super_key => {
                        // F12
                        new_active_opt = Some(12);
                    }

                    0x5B => {
                        // Super
                        self.super_key = key_event.pressed;
                    }

                    _ => (),
                },

                EventOption::Resize(resize_event) => {
                    let active_vt = self.vts.get_mut(&self.active_vt.unwrap()).unwrap();
                    active_vt.send_command(Cmd::Resize {
                        vt: active_vt.index,
                        width: resize_event.width,
                        height: resize_event.height,

                        // TODO(andypython): Figure out how to get the stride.
                        stride: resize_event.width,
                    })?;
                }

                _ => continue,
            }

            if let Some(new_active) = new_active_opt {
                if new_active == self.vts[&self.active_vt.unwrap()].index {
                    continue 'out;
                }

                if self.vts.contains_key(&new_active) {
                    let active_vt = self.vts.get_mut(&self.active_vt.unwrap()).unwrap();

                    active_vt.send_command(Cmd::Deactivate(active_vt.index))?;
                }

                if let Some(new_active) = self.vts.get_mut(&new_active) {
                    new_active.send_command(Cmd::Activate {
                        vt: new_active.index,
                    })?;
                    self.active_vt = Some(new_active.index);
                } else {
                    log::warn!("inputd: switch to non-existent VT #{new_active} was requested");
                }
            }
        }

        assert!(handle.is_producer());

        let active_vt = self.active_vt.unwrap();
        for handle in self.handles.values_mut() {
            match handle {
                Handle::Consumer {
                    pending,
                    notified,
                    vt,
                    ..
                } => {
                    if *vt != active_vt {
                        continue;
                    }

                    pending.extend_from_slice(buf);
                    *notified = false;
                }
                _ => continue,
            }
        }

        Ok(buf.len())
    }

    fn fevent(
        &mut self,
        id: usize,
        flags: syscall::EventFlags,
    ) -> syscall::Result<syscall::EventFlags> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        match handle {
            Handle::Consumer {
                ref mut events,
                ref mut notified,
                ..
            } => {
                *events = flags;
                *notified = false;
                Ok(EventFlags::empty())
            }
            Handle::Producer | Handle::Control | Handle::Display { .. } => {
                log::error!("inputd: producer, control or display tried to use an event queue");
                Err(SysError::new(EINVAL))
            }
        }
    }

    fn close(&mut self, _id: usize) -> syscall::Result<usize> {
        Ok(0)
    }
}

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    // Create the ":input" scheme.
    let socket_file: Socket<V2> = Socket::create("input")?;
    let mut scheme = InputScheme::new();

    deamon.ready().unwrap();

    loop {
        scheme.has_new_events = false;
        let Some(request) = socket_file.next_request(SignalBehavior::Restart)? else {
            // Scheme likely got unmounted
            return Ok(());
        };

        match request.kind() {
            RequestKind::Call(call_request) => {
                socket_file.write_response(
                    call_request.handle_scheme_mut(&mut scheme),
                    SignalBehavior::Restart,
                )?;
            }
            RequestKind::Cancellation(_cancellation_request) => {}
            RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => unreachable!(),
        }

        if let Some(cmd) = scheme.pending_activate.take() {
            if let Some(vt) = scheme.vts.get_mut(&cmd.vt) {
                // Failing to activate a VT is not a fatal error.
                if let Err(err) = vt.send_command(Cmd::Activate { vt: vt.index }) {
                    log::error!("inputd: failed to activate VT #{}: {err}", vt.index)
                }

                scheme.active_vt = Some(vt.index);
            } else {
                log::error!("inputd: failed to activate non-existent VT #{}", cmd.vt)
            }
        }

        if !scheme.has_new_events {
            continue;
        }

        for (id, handle) in scheme.handles.iter_mut() {
            if let Handle::Consumer {
                events,
                pending,
                ref mut notified,
                vt,
            } = handle
            {
                if pending.is_empty() || *notified || !events.contains(EventFlags::EVENT_READ) {
                    continue;
                }

                let active_vt = scheme.active_vt.unwrap();

                // The activate VT is not the same as the VT that the consumer is listening to
                // for events.
                if active_vt != *vt {
                    continue;
                }

                // Notify the consumer that we have some events to read. Yum yum.
                socket_file.post_fevent(*id, EventFlags::EVENT_READ.bits())?;

                *notified = true;
            }
        }
    }
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

fn main() {
    common::setup_logging(
        "misc",
        "inputd",
        "inputd",
        log::LevelFilter::Info,
        log::LevelFilter::Debug,
    );

    let mut args = std::env::args().skip(1);

    if let Some(val) = args.next() {
        match val.as_ref() {
            // Activates a VT.
            "-A" => {
                let vt = args.next().unwrap().parse::<usize>().unwrap();

                let mut handle =
                    inputd::ControlHandle::new().expect("inputd: failed to open display handle");
                handle
                    .activate_vt(vt)
                    .expect("inputd: failed to activate VT");
            }

            _ => panic!("inputd: invalid argument: {}", val),
        }
    } else {
        redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
    }
}
