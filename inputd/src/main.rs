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

use core::mem::size_of;
use std::collections::BTreeMap;
use std::mem::transmute;
use std::sync::atomic::{AtomicUsize, Ordering};

use inputd::{VtActivate, VtEvent, VtEventKind};

use libredox::errno::ESTALE;
use redox_scheme::{RequestKind, SchemeMut, SignalBehavior, Socket, V2};

use orbclient::{Event, EventOption};
use syscall::{Error as SysError, EventFlags, EINVAL};

enum Handle {
    Producer,
    Consumer {
        events: EventFlags,
        pending: Vec<u8>,
        /// We return an ESTALE error once to indicate that a handoff to a different graphics driver
        /// is necessary.
        needs_handoff: bool,
        notified: bool,
        vt: usize,
    },
    Display {
        events: EventFlags,
        pending: Vec<VtEvent>,
        notified: bool,
        device: String,
        /// Control of all VT's gets handed over from earlyfb devices to the first non-earlyfb device.
        is_earlyfb: bool,
    },
    Control,
}

impl Handle {
    pub fn is_producer(&self) -> bool {
        matches!(self, Handle::Producer)
    }
}

#[derive(Debug)]
struct Vt {
    display: String,
}

impl Vt {
    fn new(display: impl Into<String>) -> Self {
        Self {
            display: display.into(),
        }
    }
}

struct InputScheme {
    handles: BTreeMap<usize, Handle>,

    next_id: AtomicUsize,
    next_vt_id: AtomicUsize,

    vts: BTreeMap<usize, Vt>,
    super_key: bool,
    active_vt: Option<usize>,

    has_new_events: bool,
    maybe_perform_handoff_to: Option<String>,
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

            has_new_events: false,
            maybe_perform_handoff_to: None,
        }
    }

    fn switch_vt(&mut self, new_active: usize) -> syscall::Result<()> {
        if let Some(active_vt) = self.active_vt {
            if new_active == active_vt {
                return Ok(());
            }
        }

        if !self.vts.contains_key(&new_active) {
            log::warn!("inputd: switch to non-existent VT #{new_active} was requested");
            return Ok(());
        }

        log::info!(
            "inputd: switching from VT #{} to VT #{new_active}",
            self.active_vt.unwrap_or(0)
        );

        for handle in self.handles.values_mut() {
            match handle {
                Handle::Display {
                    pending,
                    notified,
                    device,
                    ..
                } => {
                    if let Some(active_vt) = self.active_vt {
                        if &self.vts[&active_vt].display == &*device {
                            pending.push(VtEvent {
                                kind: VtEventKind::Deactivate,
                                vt: self.active_vt.unwrap(),
                                width: 0,
                                height: 0,
                                stride: 0,
                            });
                            *notified = false;
                        }
                    }

                    if &self.vts[&new_active].display == &*device {
                        pending.push(VtEvent {
                            kind: VtEventKind::Activate,
                            vt: new_active,
                            width: 0,
                            height: 0,
                            stride: 0,
                        });
                        *notified = false;
                    }
                }
                _ => continue,
            }
        }

        self.active_vt = Some(new_active);

        Ok(())
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
                    needs_handoff: false,
                    notified: false,
                    vt: target,
                }
            }
            "handle_early" => {
                let display = path_parts.collect::<Vec<_>>().join(".");
                Handle::Display {
                    events: EventFlags::empty(),
                    pending: Vec::new(),
                    notified: false,
                    device: display,
                    is_earlyfb: true,
                }
            }
            "handle" => {
                let display = path_parts.collect::<Vec<_>>().join(".");
                self.maybe_perform_handoff_to = Some(display.clone());
                Handle::Display {
                    events: EventFlags::empty(),
                    pending: if let Some(active_vt) = self.active_vt {
                        vec![VtEvent {
                            kind: VtEventKind::Activate,
                            vt: active_vt,
                            width: 0,
                            height: 0,
                            stride: 0,
                        }]
                    } else {
                        vec![]
                    },
                    notified: false,
                    device: display,
                    is_earlyfb: false,
                }
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
            Handle::Consumer {
                pending,
                needs_handoff,
                ..
            } => {
                if *needs_handoff {
                    *needs_handoff = false;
                    // Indicates that handoff to a new graphics driver is necessary.
                    return Err(SysError::new(ESTALE));
                }

                let copy = core::cmp::min(pending.len(), buf.len());

                for (i, byte) in pending.drain(..copy).enumerate() {
                    buf[i] = byte;
                }

                Ok(copy)
            }

            Handle::Display {
                pending, device, ..
            } => {
                // FIXME Create new VT through a write instead and return a NewVt event on read
                // This allows also returning events for VT (de)activation from the display handle
                // rather than pushing them to the graphics driver.
                if buf.is_empty() {
                    // Trying to do an empty read creates a new VT.
                    let vt = self.next_vt_id.fetch_add(1, Ordering::SeqCst);
                    log::info!("inputd: created VT #{vt} for {device}");
                    self.vts.insert(vt, Vt::new(device.clone()));
                    Ok(vt)
                } else if buf.len() % size_of::<VtEvent>() == 0 {
                    let copy = core::cmp::min(pending.len(), buf.len() / size_of::<VtEvent>());

                    for (i, event) in pending.drain(..copy).enumerate() {
                        buf[i * size_of::<VtEvent>()..(i + 1) * size_of::<VtEvent>()]
                            .copy_from_slice(&unsafe {
                                transmute::<VtEvent, [u8; size_of::<VtEvent>()]>(event)
                            });
                    }
                    Ok(copy * size_of::<VtEvent>())
                } else {
                    log::error!("inputd: display tried to read incorrectly sized event");
                    return Err(SysError::new(EINVAL));
                }
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
                if buf.len() != size_of::<VtActivate>() {
                    log::error!("inputd: control tried to write incorrectly sized command");
                    return Err(SysError::new(EINVAL));
                }

                // SAFETY: We have verified the size of the buffer above.
                let cmd = unsafe { &*buf.as_ptr().cast::<VtActivate>() };

                self.switch_vt(cmd.vt)?;

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
                buf.len() / size_of::<Event>(),
            )
        };

        for event in events.iter() {
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
                    for handle in self.handles.values_mut() {
                        match handle {
                            Handle::Display {
                                pending,
                                notified,
                                device,
                                ..
                            } => {
                                if &self.vts[&self.active_vt.unwrap()].display == &*device {
                                    pending.push(VtEvent {
                                        kind: VtEventKind::Resize,
                                        vt: self.active_vt.unwrap(),
                                        width: resize_event.width,
                                        height: resize_event.height,

                                        // TODO(andypython): Figure out how to get the stride.
                                        stride: resize_event.width,
                                    });
                                    *notified = false;
                                }
                            }
                            _ => continue,
                        }
                    }
                }

                _ => continue,
            }

            if let Some(new_active) = new_active_opt {
                self.switch_vt(new_active)?;
            }
        }

        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
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
            Handle::Display {
                ref mut events,
                ref mut notified,
                ..
            } => {
                *events = flags;
                *notified = false;
                Ok(EventFlags::empty())
            }
            Handle::Producer | Handle::Control => {
                log::error!("inputd: producer or control tried to use an event queue");
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

        if let Some(display) = scheme.maybe_perform_handoff_to.take() {
            let early_displays = scheme
                .handles
                .values()
                .filter_map(|handle| match handle {
                    Handle::Display {
                        device,
                        is_earlyfb: true,
                        ..
                    } => Some(&**device),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let vts = scheme
                .vts
                .iter_mut()
                .filter_map(|(&i, vt)| {
                    if early_displays.contains(&&*vt.display) {
                        vt.display = display.clone();

                        scheme.has_new_events = true;

                        Some(i)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            for handle in scheme.handles.values_mut() {
                match handle {
                    Handle::Consumer {
                        needs_handoff,
                        notified,
                        vt,
                        ..
                    } => {
                        if !vts.contains(vt) {
                            continue;
                        }

                        *needs_handoff = true;
                        *notified = false;
                    }
                    _ => continue,
                }
            }
        }

        if !scheme.has_new_events {
            continue;
        }

        for (id, handle) in scheme.handles.iter_mut() {
            match handle {
                Handle::Consumer {
                    events,
                    pending,
                    needs_handoff,
                    ref mut notified,
                    vt,
                } => {
                    if (!*needs_handoff && pending.is_empty())
                        || *notified
                        || !events.contains(EventFlags::EVENT_READ)
                    {
                        continue;
                    }

                    let active_vt = scheme.active_vt.unwrap();

                    // The activate VT is not the same as the VT that the consumer is listening to
                    // for events.
                    if !*needs_handoff && active_vt != *vt {
                        continue;
                    }

                    // Notify the consumer that we have some events to read. Yum yum.
                    socket_file.post_fevent(*id, EventFlags::EVENT_READ.bits())?;

                    *notified = true;
                }
                Handle::Display {
                    events,
                    pending,
                    ref mut notified,
                    ..
                } => {
                    if pending.is_empty() || *notified || !events.contains(EventFlags::EVENT_READ) {
                        continue;
                    }

                    // Notify the consumer that we have some events to read. Yum yum.
                    socket_file.post_fevent(*id, EventFlags::EVENT_READ.bits())?;

                    *notified = true;
                }
                _ => {}
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
