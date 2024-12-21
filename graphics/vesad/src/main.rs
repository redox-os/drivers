#![feature(int_roundings)]
extern crate orbclient;
extern crate syscall;

use redox_scheme::{RequestKind, SignalBehavior, Socket, V2};
use std::env;
use std::fs::File;
use syscall::EVENT_READ;

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
    let socket: Socket<V2> =
        Socket::create("display.vesa").expect("vesad: failed to create display scheme");

    let mut scheme = DisplayScheme::new(framebuffers, &spec);

    let _ = File::open("/scheme/debug/disable-graphical-debug");

    let mut inputd_control_handle = inputd::ControlHandle::new().unwrap();

    libredox::call::setrens(0, 0).expect("vesad: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    inputd_control_handle.activate_vt(1).unwrap();

    let mut blocked = Vec::new();
    loop {
        let Some(request) = socket
            .next_request(SignalBehavior::Restart)
            .expect("vesad: failed to read display scheme")
        else {
            // Scheme likely got unmounted
            std::process::exit(0);
        };

        match request.kind() {
            RequestKind::Call(call_request) => {
                if let Some(resp) = call_request.handle_scheme_block_mut(&mut scheme) {
                    socket
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("vesad: failed to write display scheme");
                } else {
                    blocked.push(call_request);
                }
            }
            RequestKind::Cancellation(_cancellation_request) => {}
            RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => unreachable!(),
        }

        // If there are blocked readers, try to handle them.
        {
            let mut i = 0;
            while i < blocked.len() {
                if let Some(resp) = blocked[i].handle_scheme_block_mut(&mut scheme) {
                    socket
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("vesad: failed to write display scheme");
                    blocked.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        for (handle_id, handle) in scheme.handles.iter_mut() {
            if handle.notified_read || !handle.events.contains(EVENT_READ) {
                continue;
            }

            let can_read = if let HandleKind::Screen(vt_i, screen_i) = handle.kind {
                scheme
                    .vts
                    .get(&vt_i)
                    .and_then(|screens| screens.get(&screen_i))
                    .map_or(false, |screen| screen.can_read())
            } else {
                false
            };

            if can_read {
                handle.notified_read = true;
                socket
                    .post_fevent(*handle_id, EVENT_READ.bits())
                    .expect("vesad: failed to write display event");
            } else {
                handle.notified_read = false;
            }
        }
    }
}
