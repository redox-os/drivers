use std::env;

use redox_scheme::{RequestKind, SignalBehavior, Socket, V2};
use xhcid_interface::{ConfigureEndpointsReq, XhciClientHandle};

pub mod protocol;
pub mod scsi;

mod scheme;

use scheme::ScsiScheme;
use scsi::Scsi;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("port has to be a number");
    let protocol = args
        .next()
        .expect(USAGE)
        .parse::<u8>()
        .expect("protocol has to be a number 0-255");

    println!(
        "USB SCSI driver spawned with scheme `{}`, port {}, protocol {}",
        scheme, port, protocol
    );

    redox_daemon::Daemon::new(move |d| daemon(d, scheme, port, protocol))
        .expect("usbscsid: failed to daemonize");
}
fn daemon(daemon: redox_daemon::Daemon, scheme: String, port: usize, protocol: u8) -> ! {
    let disk_scheme_name = format!(":disk.usb-{scheme}+{port}-scsi");

    // TODO: Use eventfds.
    let handle = XhciClientHandle::new(scheme.to_owned(), port);

    daemon.ready().expect("usbscsid: failed to signal rediness");

    let desc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");

    // TODO: Perhaps the drivers should just be given the config, interface, and alternate setting
    // from xhcid.
    let (conf_desc, configuration_value, (if_desc, interface_num, alternate_setting)) = desc
        .config_descs
        .iter()
        .find_map(|config_desc| {
            let interface_desc = config_desc.interface_descs.iter().find_map(|if_desc| {
                if if_desc.class == 8 && if_desc.sub_class == 6 && if_desc.protocol == 0x50 {
                    Some((if_desc.clone(), if_desc.number, if_desc.alternate_setting))
                } else {
                    None
                }
            })?;
            Some((
                config_desc.clone(),
                config_desc.configuration_value,
                interface_desc,
            ))
        })
        .expect("Failed to find suitable configuration");

    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: configuration_value,
            interface_desc: Some(interface_num),
            alternate_setting: Some(alternate_setting),
        })
        .expect("Failed to configure endpoints");

    let mut protocol = protocol::setup(&handle, protocol, &desc, &conf_desc, &if_desc)
        .expect("Failed to setup protocol");

    // TODO: Let all of the USB drivers fork or be managed externally, and xhcid won't have to keep
    // track of all the drivers.
    let socket_fd =
        Socket::<V2>::create(&disk_scheme_name).expect("usbscsid: failed to create disk scheme");

    //libredox::call::setrens(0, 0).expect("scsid: failed to enter null namespace");
    let mut scsi = Scsi::new(&mut *protocol).expect("usbscsid: failed to setup SCSI");
    println!("SCSI initialized");
    let mut buffer = [0u8; 512];
    scsi.read(&mut *protocol, 0, &mut buffer).unwrap();
    println!("DISK CONTENT: {}", base64::encode(&buffer[..]));

    let mut scsi_scheme = ScsiScheme::new(&mut scsi, &mut *protocol);

    // TODO: Use nonblocking and put all pending calls in a todo VecDeque. Use an eventfd as well.
    loop {
        let req = match socket_fd
            .next_request(SignalBehavior::Restart)
            .expect("scsid: failed to read disk scheme")
        {
            Some(r) => {
                if let RequestKind::Call(c) = r.kind() {
                    c
                } else {
                    continue;
                }
            }
            None => break,
        };
        let resp = req.handle_scheme_mut(&mut scsi_scheme);
        socket_fd
            .write_response(resp, SignalBehavior::Restart)
            .expect("scsid: failed to write cqe");
    }

    std::process::exit(0);
}
