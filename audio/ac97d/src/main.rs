//#![deny(warnings)]

extern crate bitflags;
extern crate event;
extern crate spin;
extern crate syscall;

use std::cell::RefCell;
use std::fs::File;
use std::io::{ErrorKind, Read, Result, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use std::usize;

use event::{user_data, EventQueue};
use libredox::flag;
use pcid_interface::{PciBar, PciFunctionHandle};
use syscall::{EventFlags, Packet, SchemeBlockMut};

pub mod device;

fn main() {
    let pcid_handle =
        PciFunctionHandle::connect_default().expect("ac97d: failed to setup channel to pcid");
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ac97");

    let bar0 = pci_config.func.bars[0].expect_port();
    let bar1 = pci_config.func.bars[1].expect_port();

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ac97d: no legacy interrupts supported");

    println!(" + ac97 {}", pci_config.func.display());

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        common::setup_logging(
            "audio",
            "pcie",
            "ac97",
            log::LevelFilter::Info,
            log::LevelFilter::Info,
        );

        common::acquire_port_io_rights()
            .expect("ac97d: failed to set I/O privilege level to Ring 3");

        let mut irq_file = irq.irq_handle("ac97d");

        let mut device =
            unsafe { device::Ac97::new(bar0, bar1).expect("ac97d: failed to allocate device") };
        let socket_fd = libredox::call::open(
            ":audiohw",
            flag::O_RDWR | flag::O_CREAT | flag::O_NONBLOCK,
            0,
        )
        .expect("ac97d: failed to create hda scheme");
        let mut socket = unsafe { File::from_raw_fd(socket_fd as RawFd) };

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let mut event_queue =
            EventQueue::<Source>::new().expect("ac97d: Could not create event queue.");
        event_queue
            .subscribe(
                irq_file.as_raw_fd() as usize,
                Source::Irq,
                event::EventFlags::READ,
            )
            .unwrap();
        event_queue
            .subscribe(socket_fd, Source::Scheme, event::EventFlags::READ)
            .unwrap();

        daemon.ready().expect("ac97d: failed to signal readiness");

        libredox::call::setrens(0, 0).expect("ac97d: failed to enter null namespace");

        let mut todo = Vec::<Packet>::new();

        let all = [Source::Irq, Source::Scheme];
        'events: for event in all
            .into_iter()
            .chain(event_queue.map(|e| e.expect("ac97d: failed to get next event").user_data))
        {
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
                                socket.write(&packet).unwrap();
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
                            Err(err) => {
                                if err.kind() == ErrorKind::WouldBlock {
                                    break;
                                } else {
                                    panic!("ac97d: failed to read socket: {err}");
                                }
                            }
                        }

                        if let Some(a) = device.handle(&mut packet) {
                            packet.a = a;
                            socket.write(&packet).unwrap();
                        } else {
                            todo.push(packet);
                        }
                    }

                    /*
                    let next_read = device.borrow().next_read();
                    if next_read > 0 {
                    return Ok(Some(next_read));
                    }
                    */
                }
            }
        }

        std::process::exit(0);
    })
    .expect("ac97d: failed to daemonize");
}
