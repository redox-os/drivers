#![feature(allocator_api)]

extern crate orbclient;
extern crate syscall;

use std::{env, process, ptr};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::{RawFd, FromRawFd};
use syscall::{physmap, physunmap, Packet, SchemeMut, EVENT_READ, PHYSMAP_WRITE, PHYSMAP_WRITE_COMBINE};

use crate::scheme::{DisplayScheme, HandleKind};

pub mod display;
pub mod scheme;
pub mod screen;

fn main() {
    let mut spec = Vec::new();

    for arg in env::args().skip(1) {
        if arg == "T" {
            spec.push(false);
        } else if arg == "G" {
            spec.push(true);
        } else {
            println!("vesad: unknown screen type: {}", arg);
        }
    }

    let width = usize::from_str_radix(
            &env::var("FRAMEBUFFER_WIDTH")
                .expect("FRAMEBUFFER_WIDTH not set"),
            16
        ).expect("failed to parse FRAMEBUFFER_WIDTH");
    let height = usize::from_str_radix(
            &env::var("FRAMEBUFFER_HEIGHT")
                .expect("FRAMEBUFFER_HEIGHT not set"),
            16
        ).expect("failed to parse FRAMEBUFFER_HEIGHT");
    let physbaseptr = usize::from_str_radix(
            &env::var("FRAMEBUFFER_ADDR")
                .expect("FRAMEBUFFER_ADDR not set"),
            16
        ).expect("failed to parse FRAMEBUFFER_ADDR");

    println!("vesad: {}x{} at 0x{:X}", width, height, physbaseptr);

    if physbaseptr > 0 {
        // Daemonize
        let mut pipes = [0; 2];
        syscall::pipe2(&mut pipes, 0).unwrap();
        let mut read = unsafe { File::from_raw_fd(pipes[0] as RawFd) };
        let mut write = unsafe { File::from_raw_fd(pipes[1] as RawFd) };
        let pid = unsafe { syscall::clone(syscall::CloneFlags::empty()).unwrap() };
        if pid != 0 {
            drop(write);

            let mut res = [0];
            if read.read(&mut res).unwrap() == res.len() {
                process::exit(res[0] as i32);
            } else {
                eprintln!("vesad: daemon pipe EOF");
                process::exit(1);
            }
        } else {
            drop(read);

            let mut socket = File::create(":display").expect("vesad: failed to create display scheme");

            let size = width * height;
            //TODO: Remap on resize
            let largest_size = 8 * 1024 * 1024;
            let onscreen = unsafe { physmap(physbaseptr, largest_size * 4, PHYSMAP_WRITE | PHYSMAP_WRITE_COMBINE).expect("vesad: failed to map VBE LFB") };
            unsafe { ptr::write_bytes(onscreen as *mut u32, 0, size); }

            let mut scheme = DisplayScheme::new(width, height, onscreen, &spec);

            syscall::setrens(0, 0).expect("vesad: failed to enter null namespace");

            write.write(&[0]).unwrap();
            drop(write);

            let mut blocked = Vec::new();
            loop {
                let mut packet = Packet::default();
                if socket.read(&mut packet).expect("vesad: failed to read display scheme") == 0 {
                    //TODO: Handle blocked
                    break;
                }

                // If it is a read packet, and there is no data, block it. Otherwise, handle packet
                if packet.a == syscall::number::SYS_READ && packet.d > 0 && scheme.can_read(packet.b).is_none() {
                    blocked.push(packet);
                } else {
                    scheme.handle(&mut packet);
                    socket.write(&packet).expect("vesad: failed to write display scheme");
                }

                // If there are blocked readers, and data is available, handle them
                {
                    let mut i = 0;
                    while i < blocked.len() {
                        if scheme.can_read(blocked[i].b).is_some() {
                            let mut packet = blocked.remove(i);
                            scheme.handle(&mut packet);
                            socket.write(&packet).expect("vesad: failed to write display scheme");
                        } else {
                            i += 1;
                        }
                    }
                }

                for (handle_id, handle) in scheme.handles.iter_mut() {
                    if !handle.events.contains(EVENT_READ) {
                        continue;
                    }

                    // Can't use scheme.can_read() because we borrow handles as mutable.
                    // (and because it'd treat O_NONBLOCK sockets differently)
                    let count = if let HandleKind::Screen(screen_i) = handle.kind {
                        scheme.screens.get(&screen_i)
                            .and_then(|screen| screen.can_read())
                            .unwrap_or(0)
                    } else { 0 };

                    if count > 0 {
                        if !handle.notified_read {
                            handle.notified_read = true;
                            let event_packet = Packet {
                                id: 0,
                                pid: 0,
                                uid: 0,
                                gid: 0,
                                a: syscall::number::SYS_FEVENT,
                                b: *handle_id,
                                c: EVENT_READ.bits(),
                                d: count
                            };

                            socket.write(&event_packet).expect("vesad: failed to write display event");
                        }
                    } else {
                        handle.notified_read = false;
                    }
                }
            }
        }
    }
}
