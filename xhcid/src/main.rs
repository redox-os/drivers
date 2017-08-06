#[macro_use]
extern crate bitflags;
extern crate event;
extern crate plain;
extern crate syscall;

use event::EventQueue;
use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::io::{Result, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::Arc;
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::scheme::SchemeMut;

use xhci::Xhci;

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
        let socket_fd = syscall::open(":usb", syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK).expect("xhcid: failed to create usb scheme");
        let socket = Arc::new(RefCell::new(unsafe { File::from_raw_fd(socket_fd) }));

        let mut irq_file = File::open(format!("irq:{}", irq)).expect("xhcid: failed to open IRQ file");

        let address = unsafe { syscall::physmap(bar, 65536, syscall::MAP_WRITE).expect("xhcid: failed to map address") };
        {
            let mut hci = Xhci::new(address).expect("xhcid: failed to allocate device");

            hci.probe().expect("xhcid: failed to probe");

            let mut event_queue = EventQueue::<()>::new().expect("xhcid: failed to create event queue");

            let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

            //let device_irq = device.clone();
            let socket_irq = socket.clone();
            let todo_irq = todo.clone();
            event_queue.add(irq_file.as_raw_fd(), move |_count: usize| -> Result<Option<()>> {
                /*
                let mut irq = [0; 8];
                irq_file.read(&mut irq)?;

                let isr = unsafe { device_irq.borrow_mut().irq() };
                if isr != 0 {
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
                }
                */
                Ok(None)
            }).expect("xhcid: failed to catch events on IRQ file");

            let socket_fd = socket.borrow().as_raw_fd();
            let socket_packet = socket.clone();
            event_queue.add(socket_fd, move |_count: usize| -> Result<Option<()>> {
                /*
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
                */
                Ok(None)
            }).expect("xhcid: failed to catch events on scheme file");

            event_queue.trigger_all(0).expect("xhcid: failed to trigger events");

            event_queue.run().expect("xhcid: failed to handle events");
        }
        unsafe { let _ = syscall::physunmap(address); }
    }
}
