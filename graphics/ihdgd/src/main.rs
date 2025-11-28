use pcid_interface::PciFunctionHandle;
use redox_scheme::{RequestKind, SignalBehavior, Socket};

mod device;
use self::device::Device;

fn main() {
    let pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ihdg");

    common::setup_logging(
        "graphics",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    log::info!("IHDG {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let scheme_name = format!("ihdg.{}", name);
        let socket = Socket::create(&scheme_name).expect("ihdgd: failed to create scheme");

        //TODO daemon.ready().expect("ihdgd: failed to notify parent");

        let device = Device::new(&pci_config.func).expect("ihdgd: failed to initialize device");
        //log::info!("{:#X?}", device);

        libredox::call::setrens(0, 0).expect("ihdgd: failed to enter null namespace");

        loop {
            let Some(request) = socket
                .next_request(SignalBehavior::Restart)
                .expect("ihdgd: failed to get next scheme request")
            else {
                // Scheme likely got unmounted
                std::process::exit(0);
            };
            /*TODO
            match request.kind() {
                RequestKind::Call(call) => {
                    let response = call.handle_sync(&mut scheme);

                    socket
                        .write_response(response, SignalBehavior::Restart)
                        .expect("ihdgd: failed to write next scheme response");
                }
                RequestKind::OnClose { id } => {
                    scheme.on_close(id);
                }
                _ => (),
            }
            */
        }
    })
    .expect("ihdgd: failed to daemonize");
}
