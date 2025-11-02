use common::acquire_port_io_rights;
use inputd::ProducerHandle;
use pcid_interface::PciFunctionHandle;
use redox_scheme::{RequestKind, SignalBehavior, Socket};

use crate::bga::Bga;
use crate::scheme::BgaScheme;

mod bga;
mod scheme;

fn main() {
    let pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_bga");

    common::setup_logging(
        "graphics",
        "pci",
        &name,
        log::LevelFilter::Warn,
        log::LevelFilter::Info,
    );

    log::info!(" + BGA {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        acquire_port_io_rights().expect("bgad: failed to get port IO permission");

        let socket = Socket::create("bga").expect("bgad: failed to create bga scheme");

        let mut bga = Bga::new();
        log::info!("   - BGA {}x{}", bga.width(), bga.height());

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
                .expect("bgad: failed to get next scheme request")
            else {
                // Scheme likely got unmounted
                std::process::exit(0);
            };
            match request.kind() {
                RequestKind::Call(call) => {
                    let response = call.handle_sync(&mut scheme);

                    socket
                        .write_response(response, SignalBehavior::Restart)
                        .expect("bgad: failed to write next scheme response");
                }
                RequestKind::OnClose { id } => {
                    scheme.on_close(id);
                }
                _ => (),
            }
        }
    })
    .expect("bgad: failed to daemonize");
}
