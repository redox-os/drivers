//#![deny(warnings)]

extern crate bitflags;
extern crate spin;
extern crate syscall;
extern crate event;

use std::{env, usize};
use std::fs::File;
use std::io::{ErrorKind, Read, Write, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use libredox::flag;
use syscall::{Packet, SchemeBlockMut, EventFlags};
use std::cell::RefCell;
use std::sync::Arc;

use event::EventQueue;
use redox_log::{OutputBuilder, RedoxLogger};

pub mod device;

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("audio", "pcie", "sb16.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("sb16d: failed to create sb16.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("audio", "pcie", "sb16.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("sb16d: failed to create sb16.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("sb16d: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("sb16d: failed to set default logger: {}", error);
            None
        }
    }
}

fn main() {
    let mut args = env::args().skip(1);

    let addr_str = args.next().unwrap_or("220".to_string());
    let addr = u16::from_str_radix(&addr_str, 16).expect("sb16: failed to parse address");

    println!(" + sb16 at 0x{:X}\n", addr);

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        let _logger_ref = setup_logging();

        common::acquire_port_io_rights().expect("sb16d: failed to acquire port IO rights");

        let device = Arc::new(RefCell::new(unsafe { device::Sb16::new(addr).expect("sb16d: failed to allocate device") }));
        let socket_fd = libredox::call::open(":audiohw", flag::O_RDWR | flag::O_CREAT | flag::O_NONBLOCK, 0).expect("sb16d: failed to create hda scheme");
        let socket = Arc::new(RefCell::new(unsafe { File::from_raw_fd(socket_fd as RawFd) }));

        //TODO: error on multiple IRQs?
        let mut irq_file = match device.borrow().irqs.first() {
            Some(irq) => File::open(format!("irq:{}", irq)).expect("sb16d: failed to open IRQ file"),
            None => panic!("sb16d: no IRQs found"),
        };

        daemon.ready().expect("sb16d: failed to signal readiness");

        let mut event_queue = EventQueue::<usize>::new().expect("sb16d: Could not create event queue.");

        libredox::call::setrens(0, 0).expect("sb16d: failed to enter null namespace");

        let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

        let todo_irq = todo.clone();
        let device_irq = device.clone();
        let socket_irq = socket.clone();

        // TODO: Use new event queue API

        event_queue.add(irq_file.as_raw_fd(), move |_event| -> Result<Option<usize>> {
            let mut irq = [0; 8];
            irq_file.read(&mut irq)?;

            if device_irq.borrow_mut().irq() {
                irq_file.write(&mut irq)?;

                let mut todo = todo_irq.borrow_mut();
                let mut i = 0;
                while i < todo.len() {
                    if let Some(a) = device_irq.borrow_mut().handle(&mut todo[i]) {
                        let mut packet = todo.remove(i);
                        packet.a = a;
                        socket_irq.borrow_mut().write(&packet)?;
                    } else {
                        i += 1;
                    }
                }

                /*
                let next_read = device_irq.next_read();
                if next_read > 0 {
                    return Ok(Some(next_read));
                }
                */
            }
            Ok(None)
        }).expect("sb16d: failed to catch events on IRQ file");
        let socket_fd = socket.borrow().as_raw_fd();
        let socket_packet = socket.clone();
        event_queue.add(socket_fd, move |_event| -> Result<Option<usize>> {
            loop {
                let mut packet = Packet::default();
                match socket_packet.borrow_mut().read(&mut packet) {
                    Ok(0) => return Ok(Some(0)),
                    Ok(_) => (),
                    Err(err) => if err.kind() == ErrorKind::WouldBlock {
                        break;
                    } else {
                        return Err(err);
                    }
                }

                if let Some(a) = device.borrow_mut().handle(&mut packet) {
                    packet.a = a;
                    socket_packet.borrow_mut().write(&packet)?;
                } else {
                    todo.borrow_mut().push(packet);
                }
            }

            /*
            let next_read = device.borrow().next_read();
            if next_read > 0 {
                return Ok(Some(next_read));
            }
            */

            Ok(None)
        }).expect("sb16d: failed to catch events on IRQ file");

        for event_count in event_queue.trigger_all(event::Event {
            fd: 0,
            flags: EventFlags::empty(),
        }).expect("sb16d: failed to trigger events") {
            socket.borrow_mut().write(&Packet {
                id: 0,
                pid: 0,
                uid: 0,
                gid: 0,
                a: syscall::number::SYS_FEVENT,
                b: 0,
                c: syscall::flag::EVENT_READ.bits(),
                d: event_count
            }).expect("sb16d: failed to write event");
        }

        loop {
            {
                //device_loop.borrow_mut().handle_interrupts();
            }
            let event_count = event_queue.run().expect("sb16d: failed to handle events");
            if event_count == 0 {
                //TODO: Handle todo
                break;
            }

            socket.borrow_mut().write(&Packet {
                id: 0,
                pid: 0,
                uid: 0,
                gid: 0,
                a: syscall::number::SYS_FEVENT,
                b: 0,
                c: syscall::flag::EVENT_READ.bits(),
                d: event_count
            }).expect("sb16d: failed to write event");
        }

        std::process::exit(0);
    }).expect("sb16d: failed to daemonize");
}
