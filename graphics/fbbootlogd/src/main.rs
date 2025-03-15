//! Fbbootlogd renders the boot log and presents it on VT1.
//!
//! While fbbootlogd is superficially similar to fbcond, the major difference is:
//!
//! * Fbbootlogd doesn't accept input coming from the keyboard. It only allows getting written to.
//!
//! In the future fbbootlogd may also pull from logd as opposed to have logd push logs to it. And it
//! it could display a boot splash like plymouth instead of a boot log when booting in quiet mode.

use std::os::fd::AsRawFd;

use event::EventQueue;
use inputd::ConsumerHandleEvent;
use libredox::errno::EAGAIN;
use orbclient::Event;
use redox_scheme::{RequestKind, SignalBehavior, Socket};

use crate::scheme::FbbootlogScheme;

mod display;
mod scheme;

fn main() {
    redox_daemon::Daemon::new(|daemon| inner(daemon)).expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon) -> ! {
    let event_queue = EventQueue::new().expect("fbbootlogd: failed to create event queue");

    event::user_data! {
        enum Source {
            Scheme,
            Input,
        }
    }

    let socket =
        Socket::nonblock("fbbootlog").expect("fbbootlogd: failed to create fbbootlog scheme");

    event_queue
        .subscribe(
            socket.inner().raw(),
            Source::Scheme,
            event::EventFlags::READ,
        )
        .expect("fbcond: failed to subscribe to scheme events");

    let mut scheme = FbbootlogScheme::new();

    event_queue
        .subscribe(
            scheme.display.input_handle.event_handle().as_raw_fd() as usize,
            Source::Input,
            event::EventFlags::READ,
        )
        .expect("fbbootlogd: failed to subscribe to scheme events");

    // This is not possible for now as fbbootlogd needs to open new displays at runtime for graphics
    // driver handoff. In the future inputd may directly pass a handle to the display instead.
    //libredox::call::setrens(0, 0).expect("fbbootlogd: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    for event in event_queue {
        match event.expect("fbbootlogd: failed to get event").user_data {
            Source::Scheme => {
                loop {
                    let request = match socket.next_request(SignalBehavior::Restart) {
                        Ok(Some(request)) => request,
                        Ok(None) => {
                            // Scheme likely got unmounted
                            std::process::exit(0);
                        }
                        Err(err) if err.errno == EAGAIN => break,
                        Err(err) => panic!("fbbootlogd: failed to read display scheme: {err:?}"),
                    };

                    match request.kind() {
                        RequestKind::Call(call) => {
                            let response = call.handle_scheme(&mut scheme);

                            socket
                                .write_responses(&[response], SignalBehavior::Restart)
                                .expect("pcid: failed to write next scheme response");
                        }
                        RequestKind::OnClose { id } => {
                            scheme.on_close(id);
                        }
                        _ => (),
                    }
                }
            }
            Source::Input => {
                let mut events = [Event::new(); 16];
                loop {
                    match scheme
                        .display
                        .input_handle
                        .read_events(&mut events)
                        .expect("fbbootlogd: error while reading events")
                    {
                        ConsumerHandleEvent::Events(&[]) => break,
                        ConsumerHandleEvent::Events(_) => {}
                        ConsumerHandleEvent::Handoff => {
                            scheme.display.handle_handoff();
                        }
                    }
                }
            }
        }
    }

    std::process::exit(0);
}
