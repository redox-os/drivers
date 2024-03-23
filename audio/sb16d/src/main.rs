//#![deny(warnings)]

use std::{env, usize};
use libredox::errno::{EAGAIN, EWOULDBLOCK};
use libredox::{flag, Fd};
use syscall::{Packet, SchemeBlockMut};

use event::{user_data, EventQueue};
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

        let mut device = unsafe { device::Sb16::new(addr).expect("sb16d: failed to allocate device") };
        let socket = Fd::open(":audiohw", flag::O_RDWR | flag::O_CREAT | flag::O_NONBLOCK, 0).expect("sb16d: failed to create hda scheme");

        //TODO: error on multiple IRQs?
        let irq_file = match device.irqs.first() {
            Some(irq) => Fd::open(&format!("irq:{}", irq), flag::O_RDWR, 0).expect("sb16d: failed to open IRQ file"),
            None => panic!("sb16d: no IRQs found"),
        };
        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let event_queue = EventQueue::<Source>::new().expect("sb16d: Could not create event queue.");
        event_queue.subscribe(irq_file.raw(), Source::Irq, event::EventFlags::READ).unwrap();
        event_queue.subscribe(socket.raw(), Source::Scheme, event::EventFlags::READ).unwrap();

        daemon.ready().expect("sb16d: failed to signal readiness");

        libredox::call::setrens(0, 0).expect("sb16d: failed to enter null namespace");

        let mut todo = Vec::<Packet>::new();

        let all = [Source::Irq, Source::Scheme];

        'events: for event in all.into_iter().chain(event_queue.map(|e| e.expect("sb16d: failed to get next event").user_data)) {
            match event {
                Source::Irq => {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();

                    if device.irq() {
                        irq_file.write(&mut irq).unwrap();

                        let mut i = 0;
                        while i < todo.len() {
                            if let Some(a) = device.handle(&mut todo[i]) {
                                let mut packet = todo.remove(i);
                                packet.a = a;
                                socket.write(&packet).expect("sb16d: failed to write to socket");
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
                }
                Source::Scheme => {
                    loop {
                        let mut packet = Packet::default();
                        match socket.read(&mut packet) {
                            Ok(0) => break 'events,
                            Ok(_) => (),
                            Err(err) => if err.errno() == EWOULDBLOCK || err.errno() == EAGAIN {
                                break;
                            } else {
                                panic!("sb16d: failed to read from scheme socket");
                            }
                        }

                        if let Some(a) = device.handle(&mut packet) {
                            packet.a = a;
                            socket.write(&packet).expect("sb16d: failed to write to scheme socket");
                        } else {
                            todo.push(packet);
                        }
                    }
                }
            }
        }

        std::process::exit(0);
    }).expect("sb16d: failed to daemonize");
}
