#![feature(int_roundings)]
extern crate orbclient;
extern crate syscall;

use event::{user_data, EventQueue};
use libredox::errno::{EAGAIN, EOPNOTSUPP};
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket};
use std::env;
use std::fs::File;
use std::os::fd::AsRawFd;

use crate::{framebuffer::FrameBuffer, scheme::DisplayScheme};

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
    let socket = Socket::nonblock("display.vesa").expect("vesad: failed to create display scheme");

    let mut scheme = DisplayScheme::new(framebuffers, &spec);

    let mut inputd_control_handle = inputd::ControlHandle::new().unwrap();

    user_data! {
        enum Source {
            Input,
            Scheme,
        }
    }

    let event_queue: EventQueue<Source> =
        EventQueue::new().expect("vesad: failed to create event queue");
    event_queue
        .subscribe(
            scheme.inputd_handle.inner().as_raw_fd() as usize,
            Source::Input,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            socket.inner().raw(),
            Source::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    let _ = File::open("/scheme/debug/disable-graphical-debug");

    libredox::call::setrens(0, 0).expect("vesad: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    inputd_control_handle.activate_vt(1).unwrap();

    let all = [Source::Input, Source::Scheme];
    for event in all
        .into_iter()
        .chain(event_queue.map(|e| e.expect("vesad: failed to get next event").user_data))
    {
        match event {
            Source::Input => {
                while let Some(vt_event) = scheme
                    .inputd_handle
                    .read_vt_event()
                    .expect("vesad: failed to read display handle")
                {
                    scheme.handle_vt_event(vt_event);
                }
            }
            Source::Scheme => {
                loop {
                    let request = match socket.next_request(SignalBehavior::Restart) {
                        Ok(Some(request)) => request,
                        Ok(None) => {
                            // Scheme likely got unmounted
                            std::process::exit(0);
                        }
                        Err(err) if err.errno == EAGAIN => break,
                        Err(err) => panic!("vesad: failed to read display scheme: {err}"),
                    };

                    match request.kind() {
                        RequestKind::Call(call_request) => {
                            socket
                                .write_response(
                                    call_request.handle_scheme(&mut scheme),
                                    SignalBehavior::Restart,
                                )
                                .expect("vesad: failed to write display scheme");
                        }
                        RequestKind::SendFd(sendfd_request) => {
                            socket
                                .write_response(
                                    Response::for_sendfd(
                                        &sendfd_request,
                                        Err(syscall::Error::new(EOPNOTSUPP)),
                                    ),
                                    SignalBehavior::Restart,
                                )
                                .expect("vesad: failed to write scheme");
                        }
                        RequestKind::Cancellation(_cancellation_request) => {}
                        RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                            unreachable!()
                        }
                    }
                }
            }
        }
    }

    panic!();
}
