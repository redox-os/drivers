use std::env;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args.next().expect(USAGE);
    let protocol = args.next().expect(USAGE);

    println!("USB SCSI driver spawned with scheme `{}`, port {}, protocol {}", scheme, port, protocol);
}
