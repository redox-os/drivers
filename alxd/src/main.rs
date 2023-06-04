#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(unused_parens)]
#![feature(concat_idents)]

extern crate event;
extern crate netutils;
extern crate syscall;

use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::io::{Read, Write, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;

use event::EventQueue;
use syscall::{EventFlags, Packet, SchemeMut, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::error::EWOULDBLOCK;

pub mod device;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("alxd: no name provided");
    name.push_str("_alx");

    let bar_str = args.next().expect("alxd: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("alxd: failed to parse address");

    let irq_str = args.next().expect("alxd: no irq provided");
    let irq = irq_str.parse::<u8>().expect("alxd: failed to parse irq");

    print!("{}", format!(" + ALX {} on: {:X}, IRQ: {}\n", name, bar, irq));

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        let socket_fd = syscall::open(":network", syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK).expect("alxd: failed to create network scheme");
        let socket = Arc::new(RefCell::new(unsafe { File::from_raw_fd(socket_fd as RawFd) }));

        daemon.ready().expect("alxd: failed to signal readiness");

        let mut irq_file = File::open(format!("irq:{}", irq)).expect("alxd: failed to open IRQ file");

        let address = unsafe { syscall::physmap(bar, 128*1024, PHYSMAP_WRITE | PHYSMAP_NO_CACHE).expect("alxd: failed to map address") };
        {
            let device = Arc::new(RefCell::new(unsafe { device::Alx::new(address).expect("alxd: failed to allocate device") }));

            let mut event_queue = EventQueue::<usize>::new().expect("alxd: failed to create event queue");

            syscall::setrens(0, 0).expect("alxd: failed to enter null namespace");

            let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

            let device_irq = device.clone();
            let socket_irq = socket.clone();
            let todo_irq = todo.clone();
            event_queue.add(irq_file.as_raw_fd(), move |_event| -> Result<Option<usize>> {
                let mut irq = [0; 8];
                irq_file.read(&mut irq)?;
                if unsafe { device_irq.borrow_mut().intr_legacy() } {
                    irq_file.write(&mut irq)?;

                    let mut todo = todo_irq.borrow_mut();
                    let mut i = 0;
                    while i < todo.len() {
                        let a = todo[i].a;
                        device_irq.borrow_mut().handle(&mut todo[i]);
                        if todo[i].a == (-EWOULDBLOCK) as usize {
                            todo[i].a = a;
                            i += 1;
                        } else {
                            socket_irq.borrow_mut().write(&mut todo[i])?;
                            todo.remove(i);
                        }
                    }

                    let next_read = device_irq.borrow().next_read();
                    if next_read > 0 {
                        return Ok(Some(next_read));
                    }
                }
                Ok(None)
            }).expect("alxd: failed to catch events on IRQ file");

            let socket_packet = socket.clone();
            event_queue.add(socket_fd as RawFd, move |_event| -> Result<Option<usize>> {
                loop {
                    let mut packet = Packet::default();
                    if socket_packet.borrow_mut().read(&mut packet)? == 0 {
                        break;
                    }

                    let a = packet.a;
                    device.borrow_mut().handle(&mut packet);
                    if packet.a == (-EWOULDBLOCK) as usize {
                        packet.a = a;
                        todo.borrow_mut().push(packet);
                    } else {
                        socket_packet.borrow_mut().write(&mut packet)?;
                    }
                }

                let next_read = device.borrow().next_read();
                if next_read > 0 {
                    return Ok(Some(next_read));
                }

                Ok(None)
            }).expect("alxd: failed to catch events on IRQ file");

            for event_count in event_queue.trigger_all(event::Event {
                fd: 0,
                flags: EventFlags::empty(),
            }).expect("alxd: failed to trigger events") {
                socket.borrow_mut().write(&Packet {
                    id: 0,
                    pid: 0,
                    uid: 0,
                    gid: 0,
                    a: syscall::number::SYS_FEVENT,
                    b: 0,
                    c: syscall::flag::EVENT_READ.bits(),
                    d: event_count
                }).expect("alxd: failed to write event");
            }

            loop {
                let event_count = event_queue.run().expect("alxd: failed to handle events");

                socket.borrow_mut().write(&Packet {
                    id: 0,
                    pid: 0,
                    uid: 0,
                    gid: 0,
                    a: syscall::number::SYS_FEVENT,
                    b: 0,
                    c: syscall::flag::EVENT_READ.bits(),
                    d: event_count
                }).expect("alxd: failed to write event");
            }
        }
        std::process::exit(0);
    }).expect("alxd: failed to daemonize");
}
