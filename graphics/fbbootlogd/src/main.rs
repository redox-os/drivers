//! Fbbootlogd renders the boot log and presents it on VT1.
//!
//! While fbbootlogd is superficially similar to fbcond, the major difference is:
//!
//! * Fbbootlogd doesn't accept input coming from the keyboard. It only allows getting written to.
//!
//! In the future fbbootlogd may also pull from logd as opposed to have logd push logs to it. And it
//! it could display a boot splash like plymouth instead of a boot log when booting in quiet mode.

use redox_scheme::{RequestKind, SignalBehavior, Socket};

use crate::scheme::FbbootlogScheme;

mod display;
mod scheme;

fn main() {
    redox_daemon::Daemon::new(|daemon| inner(daemon)).expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon) -> ! {
    let socket =
        Socket::create("fbbootlog").expect("fbbootlogd: failed to create fbbootlog scheme");

    let mut scheme = FbbootlogScheme::new();

    // This is not possible for now as fbbootlogd needs to open new displays at runtime for graphics
    // driver handoff. In the future inputd may directly pass a handle to the display instead.
    //libredox::call::setrens(0, 0).expect("fbbootlogd: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

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
