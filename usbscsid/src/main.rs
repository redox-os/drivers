use std::env;

use xhcid_interface::XhciClientHandle;

pub mod protocol;
pub mod scsi;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args.next().expect(USAGE).parse::<usize>().expect("port has to be a number");
    let protocol = args.next().expect(USAGE).parse::<u8>().expect("protocol has to be a number 0-255");

    println!("USB SCSI driver spawned with scheme `{}`, port {}, protocol {}", scheme, port, protocol);

    let handle = XhciClientHandle::new(scheme, port);
    let protocol = protocol::setup(&handle, protocol);
    
}
