use inputd::ProducerHandle;
use pcid_interface::PciFunctionHandle;
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket};
use syscall::call::iopl;
use syscall::EOPNOTSUPP;

use crate::bga::Bga;
use crate::scheme::BgaScheme;

mod bga;
mod scheme;

fn main() {
    let pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_bga");

    println!(" + BGA {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        unsafe { iopl(3).unwrap() };

        let socket = Socket::create("bga").expect("bgad: failed to create bga scheme");

        let mut bga = Bga::new();
        println!("   - BGA {}x{}", bga.width(), bga.height());

        let mut scheme = BgaScheme {
            bga,
            display: ProducerHandle::new().ok(),
        };

        scheme.update_size();

        libredox::call::setrens(0, 0).expect("bgad: failed to enter null namespace");

        daemon.ready().expect("bgad: failed to notify parent");

        loop {
            let Some(request) = socket
                .next_request(SignalBehavior::Restart)
                .expect("bgad: failed to read scheme")
            else {
                // Scheme likely got unmounted
                std::process::exit(0);
            };

            match request.kind() {
                RequestKind::Call(call_request) => {
                    let resp = call_request.handle_scheme(&mut scheme);
                    socket
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("bgad: failed to write display scheme");
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
                        .expect("bgad: failed to write response");
                }
                RequestKind::Cancellation(_cancellation_request) => {}
                RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                    unreachable!()
                }
            }
        }
    })
    .expect("bgad: failed to daemonize");
}
