use std::env;

use xhcid_interface::{DevDesc, PortReqRecipient, XhciClientHandle};

mod report_desc;
mod reqs;

use report_desc::{ReportFlatIter, ReportIter, REPORT_DESC_TY};

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbhidd <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("Expected integer as input of port");
    let protocol = args.next().expect(USAGE);

    println!(
        "USB HID driver spawned with scheme `{}`, port {}, protocol {}",
        scheme, port, protocol
    );

    let handle = XhciClientHandle::new(scheme, port);
    let dev_desc: DevDesc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");
    let hid_desc = dev_desc.config_descs[0].interface_descs[0].hid_descs[0];

    // TODO: Currently it's assumed that config 0 and interface 0 are used.

    let interface_num = 0;
    let report_desc_len = hid_desc.desc_len;
    assert_eq!(hid_desc.desc_ty, REPORT_DESC_TY);

    let mut report_desc_bytes = vec![0u8; report_desc_len as usize];
    handle
        .get_descriptor(
            PortReqRecipient::Interface,
            REPORT_DESC_TY,
            0,
            interface_num,
            &mut report_desc_bytes,
        )
        .expect("Failed to retrieve report descriptor");

    let iterator = ReportIter::new(ReportFlatIter::new(&report_desc_bytes));

    for item in iterator {
        println!("HID ITEM {:?}", item);
    }
}
