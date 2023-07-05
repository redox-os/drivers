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
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use syscall::{Error as SysError, EventFlags, Packet, SchemeMut, EINVAL};

#[derive(Debug)]
enum Handle {
    Producer,
    Consumer {
        events: EventFlags,
        pending: Vec<u8>,
        notified: bool
    },
}

impl Handle {
    pub fn is_producer(&self) -> bool {
        matches!(self, Handle::Producer)
    }
}

struct InputScheme {
    handles: BTreeMap<usize, Handle>,
    next_id: AtomicUsize,
}

impl InputScheme {
    pub fn new() -> Self {
        Self {
            next_id: AtomicUsize::new(0),
            handles: BTreeMap::new(),
        }
    }
}

impl SchemeMut for InputScheme {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        let handle_ty = match path {
            "producer" => Handle::Producer,
            "consumer" => Handle::Consumer {
                events: EventFlags::empty(),
                pending: Vec::new(),
                notified: false
            },

            _ => unreachable!("inputd: invalid path {path}"),
        };

        log::info!("inputd: {path} channel has been opened");

        let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.handles.insert(fd, handle_ty);

        Ok(fd)
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        if let Handle::Consumer { pending, .. } = handle {
            let copy = core::cmp::min(pending.len(), buf.len());

            for (i, byte) in pending.drain(..copy).enumerate() {
                buf[i] = byte;
            }

            Ok(copy)
        } else {
            // A producer cannot read from the channel.
            unreachable!()
        }
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        if buf.len() == 1 && buf[0] > 0xf4 {
            return Ok(1);
        }

        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
        assert!(handle.is_producer());

        for handle in self.handles.values_mut() {
            match handle {
                Handle::Consumer {
                    ref mut pending, ref mut notified, ..
                } => {
                    pending.extend_from_slice(buf);
                    *notified = false;
                },
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
            Handle::Consumer { ref mut events, ref mut notified, .. } => {
                *events = flags;
            *notified = false;},
            _ => unreachable!(),
        }

        Ok(EventFlags::empty())
    }

    fn close(&mut self, _id: usize) -> syscall::Result<usize> {
        todo!()
    }
}

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    // Create the ":input" scheme.
    let mut socket_file = File::create(":input")?;
    let mut scheme = InputScheme::new();

    deamon.ready().unwrap();

    loop {
        let mut should_handle = false;
        let mut packet = Packet::default();
        socket_file.read(&mut packet)?;

        // The producer has written to the channel; the consumers should be notified.
        if packet.a == syscall::SYS_WRITE {
            should_handle = true;
        }

        scheme.handle(&mut packet);
        socket_file.write(&packet)?;

        if !should_handle {
            continue;
        }

        for (id, handle) in scheme.handles.iter_mut() {
            if let Handle::Consumer { events, pending, ref mut notified } = handle {
                if pending.is_empty() || *notified || !events.contains(EventFlags::EVENT_READ) {
                    continue;
                }

                // Notify the consumer that we have some events to read. Yum yum.
                let mut event_packet = Packet::default();
                event_packet.a = syscall::SYS_FEVENT;
                event_packet.b = *id;
                event_packet.c = EventFlags::EVENT_READ.bits();
                // Specifies the number of bytes that can be read non-blocking.
                event_packet.d = pending.len(); 
                socket_file.write(&event_packet)?;

                *notified = true;
            }
        }
    }
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

#[cfg(target_os = "redox")]
pub fn setup_logging(level: log::LevelFilter, name: &str) {
    use redox_log::{OutputBuilder, RedoxLogger};

    let mut logger = RedoxLogger::new().with_output(
        OutputBuilder::stderr()
            .with_filter(level)
            .with_ansi_escape_codes()
            .flush_on_newline(true)
            .build(),
    );

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", format!("{name}.log")) {
        Ok(builder) => {
            logger = logger.with_output(builder.with_filter(level).flush_on_newline(true).build())
        }
        Err(err) => eprintln!("inputd: failed to create log: {}", err),
    }

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", format!("{name}.ansi.log")) {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(level)
                    .with_ansi_escape_codes()
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(err) => eprintln!("inputd: failed to create ANSI log: {}", err),
    }

    logger.enable().unwrap();
    log::info!("inputd: enabled logger");
}

pub fn main() {
    #[cfg(target_os = "redox")]
    setup_logging(log::LevelFilter::Trace, "inputd");
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
