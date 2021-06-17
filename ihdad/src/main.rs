//#![deny(warnings)]
#![feature(llvm_asm)]

extern crate bitflags;
extern crate spin;
extern crate syscall;
extern crate event;

use std::{env, usize};
use std::fs::File;
use std::io::{ErrorKind, Read, Write, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use syscall::{CloneFlags, PHYSMAP_NO_CACHE, PHYSMAP_WRITE, Packet, SchemeBlockMut, EventFlags};
use std::cell::RefCell;
use std::sync::Arc;

use event::EventQueue;

pub mod hda;

/*
                 VEND:PROD
    Virtualbox   8086:2668
    QEMU ICH9    8086:293E
    82801H ICH8  8086:284B
*/

fn main() {
	let mut args = env::args().skip(1);

	let mut name = args.next().expect("ihda: no name provided");
	name.push_str("_ihda");

	let bar_str = args.next().expect("ihda: no address provided");
	let bar = usize::from_str_radix(&bar_str, 16).expect("ihda: failed to parse address");

    let bar_size_str = args.next().expect("ihda: no address size provided");
    let bar_size = usize::from_str_radix(&bar_size_str, 16).expect("ihda: failed to parse address size");

	let irq_str = args.next().expect("ihda: no irq provided");
	let irq = irq_str.parse::<u8>().expect("ihda: failed to parse irq");

	let vend_str = args.next().expect("ihda: no vendor id provided");
	let vend = usize::from_str_radix(&vend_str, 16).expect("ihda: failed to parse vendor id");

	let prod_str = args.next().expect("ihda: no product id provided");
	let prod = usize::from_str_radix(&prod_str, 16).expect("ihda: failed to parse product id");

	print!("{}", format!(" + ihda {} on: {:X} size: {} IRQ: {}\n", name, bar, bar_size, irq));

	// Daemonize
	if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } == 0 {
		let address = unsafe {
			syscall::physmap(bar, bar_size, PHYSMAP_WRITE | PHYSMAP_NO_CACHE)
				.expect("ihdad: failed to map address")
		};
		{
			let mut irq_file = File::open(format!("irq:{}", irq)).expect("IHDA: failed to open IRQ file");

			let vend_prod:u32 = ((vend as u32) << 16) | (prod as u32);

			let device = Arc::new(RefCell::new(unsafe { hda::IntelHDA::new(address, vend_prod).expect("ihdad: failed to allocate device") }));
			let socket_fd = syscall::open(":hda", syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK).expect("IHDA: failed to create hda scheme");
			let socket = Arc::new(RefCell::new(unsafe { File::from_raw_fd(socket_fd as RawFd) }));

			let mut event_queue = EventQueue::<usize>::new().expect("IHDA: Could not create event queue.");

            syscall::setrens(0, 0).expect("ihdad: failed to enter null namespace");

			let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

			let todo_irq = todo.clone();
			let device_irq = device.clone();
			let socket_irq = socket.clone();

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
			}).expect("IHDA: failed to catch events on IRQ file");
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
			}).expect("IHDA: failed to catch events on IRQ file");

			for event_count in event_queue.trigger_all(event::Event {
				fd: 0,
				flags: EventFlags::empty(),
			}).expect("IHDA: failed to trigger events") {
				socket.borrow_mut().write(&Packet {
					id: 0,
					pid: 0,
					uid: 0,
					gid: 0,
					a: syscall::number::SYS_FEVENT,
					b: 0,
					c: syscall::flag::EVENT_READ.bits(),
					d: event_count
				}).expect("IHDA: failed to write event");
			}

			loop {
				{
					//device_loop.borrow_mut().handle_interrupts();
				}
				let event_count = event_queue.run().expect("IHDA: failed to handle events");
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
				}).expect("IHDA: failed to write event");
			}
		}

		unsafe { let _ = syscall::physunmap(address); }
	}
}
