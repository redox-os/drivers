#![feature(if_let_guard, int_roundings)]

use std::convert::TryFrom;
use std::io::{self, prelude::*};
use std::fs::{File, OpenOptions};
use std::mem;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;

use redox_log::RedoxLogger;
use syscall::scheme::SchemeMut;

use syscall::data::{Event, Packet};
use syscall::flag::{EventFlags, O_NONBLOCK};

mod acpi;
mod scheme;
mod aml_physmem;

fn monotonic() -> (u64, u64) {
    use syscall::call::clock_gettime;
    use syscall::data::TimeSpec;
    use syscall::flag::CLOCK_MONOTONIC;

    let mut timespec = TimeSpec::default();

    clock_gettime(CLOCK_MONOTONIC, &mut timespec)
        .expect("failed to fetch monotonic time");

    (timespec.tv_sec as u64, timespec.tv_nsec as u64)
}

fn setup_logging() -> Option<&'static RedoxLogger> {
    use redox_log::OutputBuilder;

    #[allow(unused_mut)]
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("misc", "acpi", "acpid.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Warn)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create acpid.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("misc", "acpi", "acpid.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Warn)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create acpid.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("acpid: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("acpid: failed to set default logger: {}", error);
            None
        }
    }
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    setup_logging();

    let rxsdt_raw_data: Arc<[u8]> = std::fs::read("kernel.acpi:rxsdt")
        .expect("acpid: failed to read `kernel.acpi:rxsdt`")
        .into();

    let sdt = self::acpi::Sdt::new(rxsdt_raw_data)
        .expect("acpid: failed to parse [RX]SDT");

    let mut thirty_two_bit;
    let mut sixty_four_bit;

    let physaddrs_iter = match &sdt.signature {
        b"RSDT" => {
            thirty_two_bit = sdt.data().chunks(mem::size_of::<u32>())
                // TODO: With const generics, the compiler has some way of doing this for static sizes.
                .map(|chunk| <[u8; mem::size_of::<u32>()]>::try_from(chunk).unwrap())
                .map(|chunk| u32::from_le_bytes(chunk))
                .map(u64::from);

            &mut thirty_two_bit as &mut dyn Iterator<Item = u64>
        }
        b"XSDT" => {
            sixty_four_bit = sdt.data().chunks(mem::size_of::<u64>())
                .map(|chunk| <[u8; mem::size_of::<u64>()]>::try_from(chunk).unwrap())
                .map(|chunk| u64::from_le_bytes(chunk));

            &mut sixty_four_bit as &mut dyn Iterator<Item = u64>
        },
        _ => panic!("acpid: expected [RX]SDT from kernel to be either of those"),
    };

    let acpi_context = self::acpi::AcpiContext::init(physaddrs_iter);

    // TODO: I/O permission bitmap?
    common::acquire_port_io_rights().expect("acpid: failed to set I/O privilege level to Ring 3");

    let shutdown_pipe = File::open("kernel.acpi:kstop")
        .expect("acpid: failed to open `kernel.acpi:kstop`");

    let mut event_queue = OpenOptions::new()
        .write(true)
        .read(true)
        .create(false)
        .open("event:")
        .expect("acpid: failed to open event queue");

    let mut scheme_socket = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .custom_flags(O_NONBLOCK as i32)
        .open(":acpi")
        .expect("acpid: failed to open scheme socket");

    daemon.ready().expect("acpid: failed to notify parent");

    libredox::call::setrens(0, 0).expect("acpid: failed to enter null namespace");

    let _ = event_queue.write(&Event {
        id: shutdown_pipe.as_raw_fd() as usize,
        flags: EventFlags::EVENT_READ,
        data: 0,
    }).expect("acpid: failed to register shutdown pipe for event queue");

    let _ = event_queue.write(&Event {
        id: scheme_socket.as_raw_fd() as usize,
        flags: EventFlags::EVENT_READ,
        data: 1,
    }).expect("acpid: failed to register scheme socket for event queue");

    let mut scheme = self::scheme::AcpiScheme::new(&acpi_context);

    let mut event = Event::default();
    let mut packet = Packet::default();

    'events: loop {
        'packets: loop {
            let bytes_read = 'eintr1: loop {
                match scheme_socket.read(&mut packet) {
                    Ok(0) => {
                        log::info!("Terminating acpid driver, without shutting down the main system.");
                        break 'events;
                    }
                    Ok(n) => break 'eintr1 n,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue 'eintr1,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break 'packets,
                    Err(other) => {
                        log::error!("failed to read from scheme socket: {}", other);
                        break 'events;
                    }
                }
            };

            if bytes_read < mem::size_of::<Packet>() {
                log::error!("Scheme socket read less than a single packet.");
            }

            scheme.handle(&mut packet);

            let bytes_written = 'eintr2: loop {
                match scheme_socket.write(&packet) {
                    Ok(0) => {
                        log::info!("Terminating acpid driver, without shutting down the main system.");
                        break 'events;
                    }
                    Ok(n) => break 'eintr2 n,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue 'eintr2,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break 'packets,
                    Err(other) => {
                        log::error!("failed to read from scheme socket: {}", other);
                        break 'events;
                    }
                }
            };

            if bytes_written < mem::size_of::<Packet>() {
                log::error!("Scheme socket read less than a single packet.");
            }
        }

        let _ = event_queue.read(&mut event).expect("acpid: failed to read from event queue");

        if event.flags.contains(EventFlags::EVENT_READ) && event.id == shutdown_pipe.as_raw_fd() as usize {
            log::info!("Received shutdown request from kernel.");
            break 'events;
        }
        if !event.flags.contains(EventFlags::EVENT_READ) || event.id != scheme_socket.as_raw_fd() as usize {
            continue 'events;
        }
    }

    drop(shutdown_pipe);
    drop(event_queue);

    acpi_context.set_global_s_state(5);

    unreachable!("System should have shut down before this is entered");
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("acpid: failed to daemonize");
}
