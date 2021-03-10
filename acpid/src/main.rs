#![feature(renamed_spin_loop)]

use std::convert::{TryFrom, TryInto};
use std::io::prelude::*;
use std::fs::{File, OpenOptions};
use std::mem;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use redox_log::RedoxLogger;

use syscall::data::Event;
use syscall::flag::EventFlags;

mod acpi;
mod aml;

// TODO: Perhaps use the acpi and aml crates?

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

    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Trace) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("misc", "acpi", "acpid.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Trace)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create xhci.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("misc", "acpi", "acpid.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Trace)
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

fn main() {
    setup_logging();

    let rxsdt_raw_data: Arc<[u8]> = std::fs::read("kernel/acpi:rxsdt")
        .expect("acpid: failed to read `kernel/acpi:rxsdt`")
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

    for physaddr in physaddrs_iter {
        let physaddr: usize = physaddr
            .try_into()
            .expect("expected ACPI addresses to be compatible with the current word size");

        log::info!("TABLE AT {:#>08X}", physaddr);

        let sdt = self::acpi::Sdt::load_from_physical(physaddr)
            .expect("failed to load physical SDT");
        dbg!(sdt);
    }

    // TODO: I/O permission bitmap
    unsafe { syscall::iopl(3) }.expect("acpid: failed to set I/O privilege level to Ring 3");

    let shutdown_pipe = File::open("kernel/acpi:kstop")
        .expect("acpid: failed to open `kernel/acpi:kstop`");

    let mut event_queue = OpenOptions::new()
        .write(true)
        .read(true)
        .create(false)
        .open("event:")
        .expect("acpid: failed to open event queue");

    syscall::setrens(0, 0).expect("acpid: failed to enter null namespace");

    event_queue.write_all(&Event {
        id: shutdown_pipe.as_raw_fd() as usize,
        flags: EventFlags::EVENT_READ,
        data: 0,
    }).expect("acpid: failed to register shutdown pipe for event queue");

    loop {
        let mut event = Event::default();
        event_queue.read_exact(&mut event).expect("acpid: failed to read from event queue");

        if event.flags.contains(EventFlags::EVENT_READ) && event.id == shutdown_pipe.as_raw_fd() as usize {
            break;
        }
    }

    drop(shutdown_pipe);
    drop(event_queue);

    aml::set_global_s_state(todo!(), 5);
    unreachable!();
}
