//! Fbbootlogd renders the boot log and presents it on VT1.
//!
//! While fbbootlogd is superficially similar to fbcond, there are two major differences:
//!
//! * Fbbootlogd doesn't accept input coming from the keyboard. It only allows getting written to.
//! * Writing to fbbootlogd will never block. Not even on the graphics driver or inputd. This makes
//!   it safe for graphics drivers and inputd to write to the boot log without risking deadlocks.
//!   Fbcond will block on the graphics driver during handoff and will continously block on inputd
//!   to get new input. Fbbootlogd does all blocking operations in background threads such that the
//!   main thread will always keep accepting new input and writing it to the framebuffer.

use libredox::errno::EOPNOTSUPP;
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket};

use crate::scheme::FbbootlogScheme;

mod display;
mod scheme;
mod text;

fn main() {
    redox_daemon::Daemon::new(|daemon| inner(daemon)).expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon) -> ! {
    let socket =
        Socket::create("fbbootlog").expect("fbbootlogd: failed to create fbbootlog scheme");

    let mut scheme = FbbootlogScheme::new();

    let mut inputd_control_handle = inputd::ControlHandle::new().unwrap();

    // This is not possible for now as fbbootlogd needs to open new displays at runtime for graphics
    // driver handoff. In the future inputd may directly pass a handle to the display instead.
    //libredox::call::setrens(0, 0).expect("fbbootlogd: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    inputd_control_handle.activate_vt(1).unwrap();

    loop {
        let request = match socket
            .next_request(SignalBehavior::Restart)
            .expect("fbbootlogd: failed to read display scheme")
        {
            Some(request) => request,
            None => {
                // Scheme likely got unmounted
                std::process::exit(0);
            }
        };

        match request.kind() {
            RequestKind::Call(call_request) => {
                socket
                    .write_response(
                        call_request.handle_scheme(&mut scheme),
                        SignalBehavior::Restart,
                    )
                    .expect("fbbootlogd: failed to write display scheme");
            }
            RequestKind::SendFd(sendfd_request) => {
                socket
                    .write_response(
                        Response::for_sendfd(&sendfd_request, Err(syscall::Error::new(EOPNOTSUPP))),
                        SignalBehavior::Restart,
                    )
                    .expect("fbbootlogd: failed to write scheme");
            }
            RequestKind::Cancellation(_cancellation_request) => {}
            RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                unreachable!()
            }
        }
    }
}
