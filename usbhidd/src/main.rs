use std::env;

use xhcid_interface::{ConfigureEndpointsReq, DevDesc, PortReqRecipient, XhciClientHandle};

mod report_desc;
mod reqs;
mod usage_tables;

use report_desc::{MainCollectionFlags, GlobalItemsState, ReportFlatIter, ReportItem, ReportIter, ReportIterItem, REPORT_DESC_TY};
use reqs::ReportTy;

fn div_round_up(num: u32, denom: u32) -> u32 {
    if num % denom == 0 { num / denom } else { num / denom + 1 }
}

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

    let report_desc = ReportIter::new(ReportFlatIter::new(&report_desc_bytes)).collect::<Vec<_>>();

    for item in &report_desc {
        println!("{:?}", item);
    }

    handle.configure_endpoints(&ConfigureEndpointsReq { config_desc: 0 }).expect("Failed to configure endpoints");

    let (mut state, mut stack) = (GlobalItemsState::default(), Vec::new());

    let (_, application_collection) = report_desc.iter().inspect(|item: &&ReportIterItem| if let ReportIterItem::Item(ref item) = item {
        report_desc::update_state(&mut state, &mut stack, item).unwrap()
    }).filter_map(ReportIterItem::as_collection).find(|&(n, _)| n == MainCollectionFlags::Application as u8).expect("Failed to find application collection");

    // Get all main items, and their global item options.
    {
        let items = application_collection.iter().filter_map(ReportIterItem::as_item).filter_map(|item| match item {
            ReportItem::Global(_) => {
                report_desc::update_state(&mut state, &mut stack, item).unwrap();
                None
            }
            ReportItem::Main(m) => Some((state, m)),
            ReportItem::Local(_) => None,
        });
        let total_length = items.filter_map(|(state, item)| {
            let report_size = match state.report_size {
                Some(s) => s,
                None => return None,
            };
            let report_count = match state.report_count {
                Some(c) => c,
                None => return None,
            };
            let bit_length = report_size * report_count;

            if item.report_ty() != Some(ReportTy::Input) {
                return None;
            }
            Some(bit_length)
        }).sum();
        let length = div_round_up(total_length, 8);

        let mut report_buffer = vec! [0u8; length as usize];
        let mut last_buffer = report_buffer.clone();
        let report_ty = ReportTy::Input;
        let report_id = 0;

        loop {
            std::mem::swap(&mut report_buffer, &mut last_buffer);
            reqs::get_report(&handle, report_ty, report_id, interface_num, &mut report_buffer).expect("Failed to get report");
            if report_buffer != last_buffer {
                for byte in &report_buffer {
                    print!("{:#0x} ", byte);
                }
                println!();
            }
            std::thread::sleep(std::time::Duration::from_millis(10))
        }
    }

}
