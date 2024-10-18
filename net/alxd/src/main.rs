#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(unused_parens)]
#![feature(concat_idents)]

extern crate event;
extern crate syscall;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::io::{FromRawFd, RawFd};
use std::{env, iter};

use event::{user_data, EventQueue};
use libredox::flag;
use syscall::error::EWOULDBLOCK;
use syscall::{Packet, SchemeMut};

pub mod device;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("alxd: no name provided");
    name.push_str("_alx");

    let bar_str = args.next().expect("alxd: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("alxd: failed to parse address");

    let irq_str = args.next().expect("alxd: no irq provided");
    let irq = irq_str.parse::<u8>().expect("alxd: failed to parse irq");

    println!(" + ALX {} on: {:X}, IRQ: {}\n", name, bar, irq);

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        let socket_fd = libredox::call::open(
            ":network",
            flag::O_RDWR | flag::O_CREAT | flag::O_NONBLOCK,
            0,
        )
        .expect("alxd: failed to create network scheme");
        let mut socket = unsafe { File::from_raw_fd(socket_fd as RawFd) };

        daemon.ready().expect("alxd: failed to signal readiness");

        let mut irq_file =
            File::open(format!("/scheme/irq/{}", irq)).expect("alxd: failed to open IRQ file");

        let address = unsafe {
            common::physmap(
                bar,
                128 * 1024,
                common::Prot::RW,
                common::MemoryType::Uncacheable,
            )
            .expect("alxd: failed to map address") as usize
        };
        {
            let mut device =
                unsafe { device::Alx::new(address).expect("alxd: failed to allocate device") };

            user_data! {
                enum Source {
                    Irq,
                    Scheme,
                }
            }

            let event_queue =
                EventQueue::<Source>::new().expect("alxd: failed to create event queue");
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

            libredox::call::setrens(0, 0).expect("alxd: failed to enter null namespace");

            let mut todo = Vec::<Packet>::new();

            for event in iter::once(Source::Scheme)
                .chain(event_queue.map(|e| e.expect("alxd: failed to get next event").user_data))
            {
                match event {
                    Source::Irq => {
                        let mut irq = [0; 8];
                        irq_file.read(&mut irq).unwrap();
                        if unsafe { device.intr_legacy() } {
                            irq_file.write(&mut irq).unwrap();

                            let mut i = 0;
                            while i < todo.len() {
                                let a = todo[i].a;
                                device.handle(&mut todo[i]);
                                if todo[i].a == (-EWOULDBLOCK) as usize {
                                    todo[i].a = a;
                                    i += 1;
                                } else {
                                    socket
                                        .write(&mut todo[i])
                                        .expect("alxd: failed to write to socket");
                                    todo.remove(i);
                                }
                            }

                            /* TODO: Currently a no-op
                            let next_read = device.next_read();
                            if next_read > 0 {
                                return Ok(Some(next_read));
                            }
                            */
                        }
                    }
                    Source::Scheme => {
                        loop {
                            let mut packet = Packet::default();
                            if socket
                                .read(&mut packet)
                                .expect("alxd: failed read from socket")
                                == 0
                            {
                                break;
                            }

                            let a = packet.a;
                            device.handle(&mut packet);
                            if packet.a == (-EWOULDBLOCK) as usize {
                                packet.a = a;
                                todo.push(packet);
                            } else {
                                socket
                                    .write(&mut packet)
                                    .expect("alxd: failed to write to socket");
                            }
                        }

                        // TODO
                        /*
                        let next_read = device.next_read();
                        if next_read > 0 {
                            return Ok(Some(next_read));
                        }
                        */
                    }
                }
            }
        }
        std::process::exit(0);
    })
    .expect("alxd: failed to daemonize");
}
