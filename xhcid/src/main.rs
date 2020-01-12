#[macro_use]
extern crate bitflags;
extern crate event;
extern crate plain;
extern crate syscall;

use event::{Event, EventQueue};
use std::cell::RefCell;
use std::{io, env};
use std::fs::File;
use std::io::{Result, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use syscall::data::Packet;
use syscall::flag::{PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::error::EWOULDBLOCK;
use syscall::scheme::SchemeMut;

use crate::xhci::Xhci;

mod usb;
mod xhci;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

    let bar_str = args.next().expect("xhcid: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("xhcid: failed to parse address");

    let irq_str = args.next().expect("xhcid: no IRQ provided");
    let irq = irq_str.parse::<u8>().expect("xhcid: failed to parse irq");

    print!("{}", format!(" + XHCI {} on: {:X} IRQ: {}\n", name, bar, irq));

    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let socket_fd = syscall::open(format!(":usb/{}", name), syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK).expect("xhcid: failed to create usb scheme");
        let socket = Arc::new(RefCell::new(unsafe { File::from_raw_fd(socket_fd as RawFd) }));

        let mut irq_file = File::open(format!("irq:{}", irq)).expect("xhcid: failed to open IRQ file");

        let address = unsafe { syscall::physmap(bar, 65536, PHYSMAP_WRITE | PHYSMAP_NO_CACHE).expect("xhcid: failed to map address") };
        {
            let hci = Arc::new(RefCell::new(Xhci::new(address).expect("xhcid: failed to allocate device")));

            hci.borrow_mut().probe().expect("xhcid: failed to probe");

            let mut event_queue = EventQueue::<()>::new().expect("xhcid: failed to create event queue");

            syscall::setrens(0, 0).expect("xhcid: failed to enter null namespace");

            let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

            let hci_irq = hci.clone();
            let socket_irq = socket.clone();
            let todo_irq = todo.clone();
            event_queue.add(irq_file.as_raw_fd(), move |_| -> Result<Option<()>> {
                let mut irq = [0; 8];
                irq_file.read(&mut irq)?;

                if hci_irq.borrow_mut().irq() {
                    irq_file.write(&mut irq)?;

                    let mut todo = todo_irq.borrow_mut();
                    let mut i = 0;
                    while i < todo.len() {
                        let a = todo[i].a;
                        hci_irq.borrow_mut().handle(&mut todo[i]);
                        if todo[i].a == (-EWOULDBLOCK) as usize {
                            todo[i].a = a;
                            i += 1;
                        } else {
                            socket_irq.borrow_mut().write(&mut todo[i])?;
                            todo.remove(i);
                        }
                    }
                }

                Ok(None)
            }).expect("xhcid: failed to catch events on IRQ file");

            let socket_fd = socket.borrow().as_raw_fd();
            let socket_packet = socket.clone();
            event_queue.add(socket_fd, move |_| -> Result<Option<()>> {
                loop {
                    let mut packet = Packet::default();
                    match socket_packet.borrow_mut().read(&mut packet)  {
                        Ok(0) => break,
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        Ok(_) => (),
                        Err(err) => return Err(err),
                    }

                    let a = packet.a;
                    hci.borrow_mut().handle(&mut packet);
                    if packet.a == (-EWOULDBLOCK) as usize {
                        packet.a = a;
                        todo.borrow_mut().push(packet);
                    } else {
                        socket_packet.borrow_mut().write(&mut packet)?;
                    }
                }
                Ok(None)
            }).expect("xhcid: failed to catch events on scheme file");

            event_queue.trigger_all(Event {
                fd: 0,
                flags: 0
            }).expect("xhcid: failed to trigger events");

            event_queue.run().expect("xhcid: failed to handle events");
        }
        unsafe { let _ = syscall::physunmap(address); }
    }
}
