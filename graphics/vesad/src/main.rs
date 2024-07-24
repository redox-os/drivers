#![feature(int_roundings)]
extern crate orbclient;
extern crate syscall;

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use syscall::{Packet, SchemeMut, EVENT_READ};

use crate::{
    framebuffer::FrameBuffer,
    scheme::{DisplayScheme, HandleKind},
};

mod display;
mod framebuffer;
mod scheme;
mod screen;

fn main() {
    let mut spec = Vec::new();

    for _ in env::args().skip(1) {
        spec.push(());
    }

    let width = usize::from_str_radix(
        &env::var("FRAMEBUFFER_WIDTH").expect("FRAMEBUFFER_WIDTH not set"),
        16,
    )
    .expect("failed to parse FRAMEBUFFER_WIDTH");
    let height = usize::from_str_radix(
        &env::var("FRAMEBUFFER_HEIGHT").expect("FRAMEBUFFER_HEIGHT not set"),
        16,
    )
    .expect("failed to parse FRAMEBUFFER_HEIGHT");
    let phys = usize::from_str_radix(
        &env::var("FRAMEBUFFER_ADDR").expect("FRAMEBUFFER_ADDR not set"),
        16,
    )
    .expect("failed to parse FRAMEBUFFER_ADDR");
    let stride = usize::from_str_radix(
        &env::var("FRAMEBUFFER_STRIDE").expect("FRAMEBUFFER_STRIDE not set"),
        16,
    )
    .expect("failed to parse FRAMEBUFFER_STRIDE");

    println!(
        "vesad: {}x{} stride {} at 0x{:X}",
        width, height, stride, phys
    );

    if phys == 0 {
        return;
    }

    let mut framebuffers = vec![unsafe { FrameBuffer::new(phys, width, height, stride) }];

    //TODO: ideal maximum number of outputs?
    for i in 1..1024 {
        match env::var(&format!("FRAMEBUFFER{}", i)) {
            Ok(var) => match unsafe { FrameBuffer::parse(&var) } {
                Some(fb) => {
                    println!(
                        "vesad: framebuffer {}: {}x{} stride {} at 0x{:X}",
                        i, fb.width, fb.height, fb.stride, fb.phys
                    );
                    framebuffers.push(fb);
                }
                None => {
                    eprintln!("vesad: framebuffer {}: failed to parse '{}'", i, var);
                }
            },
            Err(_err) => break,
        };
    }

    redox_daemon::Daemon::new(|daemon| inner(daemon, framebuffers, &spec))
        .expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon, framebuffers: Vec<FrameBuffer>, spec: &[()]) -> ! {
    let mut socket = File::create(":display.vesa").expect("vesad: failed to create display scheme");

    let mut scheme = DisplayScheme::new(framebuffers, &spec);

    let _ = File::open("/scheme/debug/disable-graphical-debug");

    libredox::call::setrens(0, 0).expect("vesad: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    scheme.inputd_handle.activate(1).unwrap();

    let mut blocked = Vec::new();
    loop {
        let mut packet = Packet::default();
        if socket
            .read(&mut packet)
            .expect("vesad: failed to read display scheme")
            == 0
        {
            //TODO: Handle blocked
            break;
        }

        // If it is a read packet, and there is no data, block it. Otherwise, handle packet
        if packet.a == syscall::number::SYS_READ
            && packet.d > 0
            && scheme.can_read(packet.b).is_none()
        {
            blocked.push(packet);
        } else {
            scheme.handle(&mut packet);
            socket
                .write(&packet)
                .expect("vesad: failed to write display scheme");
        }

        // If there are blocked readers, and data is available, handle them
        {
            let mut i = 0;
            while i < blocked.len() {
                if scheme.can_read(blocked[i].b).is_some() {
                    let mut packet = blocked.remove(i);
                    scheme.handle(&mut packet);
                    socket
                        .write(&packet)
                        .expect("vesad: failed to write display scheme");
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
            let count = if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
                scheme
                    .vts
                    .get(&vt_i)
                    .and_then(|screens| screens.get(&screen_i))
                    .and_then(|screen| screen.can_read())
                    .unwrap_or(0)
            } else {
                0
            };

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
                        d: count,
                    };

                    socket
                        .write(&event_packet)
                        .expect("vesad: failed to write display event");
                }
            } else {
                handle.notified_read = false;
            }
        }
    }
    std::process::exit(0);
}
