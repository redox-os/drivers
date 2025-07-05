extern crate orbclient;
extern crate syscall;

use driver_graphics::GraphicsScheme;
use event::{user_data, EventQueue};
use inputd::DisplayHandle;
use std::env;
use std::os::fd::AsRawFd;

use crate::scheme::{FbAdapter, FrameBuffer};

mod scheme;

fn main() {
    if env::var("FRAMEBUFFER_WIDTH").is_err() {
        println!("vesad: No boot framebuffer");
        return;
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
        println!("vesad: Boot framebuffer at address 0");
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

    redox_daemon::Daemon::new(|daemon| inner(daemon, framebuffers))
        .expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon, framebuffers: Vec<FrameBuffer>) -> ! {
    let mut inputd_display_handle = DisplayHandle::new_early("vesa").unwrap();

    let mut scheme = GraphicsScheme::new(FbAdapter { framebuffers }, "display.vesa".to_owned());

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
            inputd_display_handle.inner().as_raw_fd() as usize,
            Source::Input,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            scheme.event_handle().raw(),
            Source::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    libredox::call::setrens(0, 0).expect("vesad: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    let all = [Source::Input, Source::Scheme];
    for event in all
        .into_iter()
        .chain(event_queue.map(|e| e.expect("vesad: failed to get next event").user_data))
    {
        match event {
            Source::Input => {
                while let Some(vt_event) = inputd_display_handle
                    .read_vt_event()
                    .expect("vesad: failed to read display handle")
                {
                    scheme.handle_vt_event(vt_event);
                }
            }
            Source::Scheme => {
                scheme
                    .tick()
                    .expect("vesad: failed to handle scheme events");
            }
        }
    }

    panic!();
}
