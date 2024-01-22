extern crate event;
extern crate netutils;
extern crate syscall;

use std::cell::RefCell;
use std::fs::File;
use std::io::{ErrorKind, Read, Result, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::process;
use std::sync::Arc;

use event::EventQueue;
use pcid_interface::PcidServerHandle;
use syscall::{EventFlags, Packet, SchemeBlockMut};

pub mod device;

fn handle_update(
    socket: &mut File,
    device: &mut device::Intel8254x,
    todo: &mut Vec<Packet>,
) -> Result<bool> {
    // Handle any blocked packets
    let mut i = 0;
    while i < todo.len() {
        if let Some(a) = device.handle(&todo[i]) {
            let mut packet = todo.remove(i);
            packet.a = a;
            socket.write(&packet)?;
        } else {
            i += 1;
        }
    }

    // Check that the socket is empty
    loop {
        let mut packet = Packet::default();
        match socket.read(&mut packet) {
            Ok(0) => return Ok(true),
            Ok(_) => (),
            Err(err) => {
                if err.kind() == ErrorKind::WouldBlock {
                    break;
                } else {
                    return Err(err);
                }
            }
        }

        if let Some(a) = device.handle(&packet) {
            packet.a = a;
            socket.write(&packet)?;
        } else {
            todo.push(packet);
        }
    }

    Ok(false)
}

fn main() {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("e1000d: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("e1000d: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_e1000");

    let (bar, bar_size) = pci_config.func.bars[0].expect_mem();

    let irq = pci_config.func.legacy_interrupt_line.expect("e1000d: no legacy interrupts supported");

    eprintln!(" + E1000 {} on: {:X} size: {} IRQ: {}", name, bar, bar_size, irq);

    redox_daemon::Daemon::new(move |daemon| {
        let socket_fd = syscall::open(
            ":network",
            syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK,
        )
        .expect("e1000d: failed to create network scheme");
        let socket = Arc::new(RefCell::new(unsafe {
            File::from_raw_fd(socket_fd as RawFd)
        }));

        let irq_fd = syscall::open(
            format!("irq:{}", irq),
            syscall::O_RDWR | syscall::O_NONBLOCK
        ).expect("e1000d: failed to open IRQ file");
        let mut irq_file = unsafe { File::from_raw_fd(irq_fd as RawFd) };

        let address = unsafe {
            common::physmap(bar, bar_size, common::Prot::RW, common::MemoryType::Uncacheable)
                .expect("e1000d: failed to map address")
        } as usize;
        {
            let device = Arc::new(RefCell::new(unsafe {
                device::Intel8254x::new(address).expect("e1000d: failed to allocate device")
            }));

            let mut event_queue =
                EventQueue::<usize>::new().expect("e1000d: failed to create event queue");

            syscall::setrens(0, 0).expect("e1000d: failed to enter null namespace");

            daemon.ready().expect("e1000d: failed to mark daemon as ready");

            let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

            let device_irq = device.clone();
            let socket_irq = socket.clone();
            let todo_irq = todo.clone();
            event_queue
                .add(
                    irq_file.as_raw_fd(),
                    move |_event| -> Result<Option<usize>> {
                        let mut irq = [0; 8];
                        irq_file.read(&mut irq)?;
                        if unsafe { device_irq.borrow().irq() } {
                            irq_file.write(&mut irq)?;

                            if handle_update(
                                &mut socket_irq.borrow_mut(),
                                &mut device_irq.borrow_mut(),
                                &mut todo_irq.borrow_mut(),
                            )? {
                                return Ok(Some(0));
                            }

                            let next_read = device_irq.borrow().next_read();
                            if next_read > 0 {
                                return Ok(Some(next_read));
                            }
                        }
                        Ok(None)
                    },
                )
                .expect("e1000d: failed to catch events on IRQ file");

            let device_packet = device.clone();
            let socket_packet = socket.clone();
            event_queue
                .add(socket_fd as RawFd, move |_event| -> Result<Option<usize>> {
                    if handle_update(
                        &mut socket_packet.borrow_mut(),
                        &mut device_packet.borrow_mut(),
                        &mut todo.borrow_mut(),
                    )? {
                        return Ok(Some(0));
                    }

                    let next_read = device_packet.borrow().next_read();
                    if next_read > 0 {
                        return Ok(Some(next_read));
                    }

                    Ok(None)
                })
                .expect("e1000d: failed to catch events on scheme file");

            let send_events = |event_count| {
                for (handle_id, _handle) in device.borrow().handles.iter() {
                    socket
                        .borrow_mut()
                        .write(&Packet {
                            id: 0,
                            pid: 0,
                            uid: 0,
                            gid: 0,
                            a: syscall::number::SYS_FEVENT,
                            b: *handle_id,
                            c: syscall::flag::EVENT_READ.bits(),
                            d: event_count,
                        })
                        .expect("e1000d: failed to write event");
                }
            };

            for event_count in event_queue
                .trigger_all(event::Event { fd: 0, flags: EventFlags::empty() })
                .expect("e1000d: failed to trigger events")
            {
                send_events(event_count);
            }

            loop {
                let event_count = event_queue.run().expect("e1000d: failed to handle events");
                if event_count == 0 {
                    //TODO: Handle todo
                    break;
                }
                send_events(event_count);
            }
        }
        process::exit(0);
    }).expect("e1000d: failed to create daemon");
}
