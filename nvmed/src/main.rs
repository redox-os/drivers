//#![deny(warnings)]
#![feature(asm)]

extern crate bitflags;
extern crate spin;
extern crate syscall;

use std::{env, usize};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use syscall::{EVENT_READ, MAP_WRITE, Event, Packet, Result, Scheme};

use self::nvme::Nvme;

mod nvme;

/*
fn create_scheme_fallback<'a>(name: &'a str, fallback: &'a str) -> Result<(&'a str, RawFd)> {
    if let Ok(fd) = syscall::open(&format!(":{}", name), syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK) {
        Ok((name, fd))
    } else {
        syscall::open(&format!(":{}", fallback), syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK)
                .map(|fd| (fallback, fd))
    }
}
*/

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("nvmed: no name provided");
    name.push_str("_nvme");

    let bar_str = args.next().expect("nvmed: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("nvmed: failed to parse address");

    let irq_str = args.next().expect("nvmed: no irq provided");
    let irq = irq_str.parse::<u8>().expect("nvmed: failed to parse irq");

    print!("{}", format!(" + NVME {} on: {:X} IRQ: {}\n", name, bar, irq));

    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let address = unsafe { syscall::physmap(bar, 4096, MAP_WRITE).expect("nvmed: failed to map address") };
        {
            let mut nvme = Nvme::new(address);
            nvme.init();
            /*
            let (_scheme_name, socket_fd) = create_scheme_fallback("disk", &name).expect("nvmed: failed to create disk scheme");
            let mut socket = unsafe { File::from_raw_fd(socket_fd) };
            syscall::fevent(socket_fd, EVENT_READ).expect("nvmed: failed to fevent disk scheme");

            let mut irq_file = File::open(&format!("irq:{}", irq)).expect("nvmed: failed to open irq file");
            let irq_fd = irq_file.as_raw_fd();
            syscall::fevent(irq_fd, EVENT_READ).expect("nvmed: failed to fevent irq file");

            let mut event_file = File::open("event:").expect("nvmed: failed to open event file");

            let scheme = DiskScheme::new(nvme::disks(address, &name));

            syscall::setrens(0, 0).expect("nvmed: failed to enter null namespace");

            loop {
                let mut event = Event::default();
                if event_file.read(&mut event).expect("nvmed: failed to read event file") == 0 {
                    break;
                }
                if event.id == socket_fd {
                    loop {
                        let mut packet = Packet::default();
                        if socket.read(&mut packet).expect("nvmed: failed to read disk scheme") == 0 {
                            break;
                        }
                        scheme.handle(&mut packet);
                        socket.write(&mut packet).expect("nvmed: failed to write disk scheme");
                    }
                } else if event.id == irq_fd {
                    let mut irq = [0; 8];
                    if irq_file.read(&mut irq).expect("nvmed: failed to read irq file") >= irq.len() {
                        //TODO : Test for IRQ
                        //irq_file.write(&irq).expect("nvmed: failed to write irq file");
                    }
                } else {
                    println!("Unknown event {}", event.id);
                }
            }
            */
        }
        unsafe { let _ = syscall::physunmap(address); }
    }
}
