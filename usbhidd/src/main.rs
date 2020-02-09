use std::env;
use std::fs::File;

use bitflags::bitflags;
use orbclient::KeyEvent as OrbKeyEvent;
use xhcid_interface::{ConfigureEndpointsReq, DevDesc, PortReqRecipient, XhciClientHandle};

mod report_desc;
mod reqs;
mod usage_tables;

use report_desc::{LocalItemsState, MainCollectionFlags, MainItem, MainItemFlags, GlobalItemsState, ReportFlatIter, ReportItem, ReportIter, ReportIterItem, REPORT_DESC_TY};
use reqs::ReportTy;

fn div_round_up(num: u32, denom: u32) -> u32 {
    if num % denom == 0 { num / denom } else { num / denom + 1 }
}

struct BinaryView<'a> {
    data: &'a [u8],
    offset: usize,
    len: usize,
}
impl<'a> BinaryView<'a> {
    pub fn new(data: &'a [u8], offset: usize, len: usize) -> Self {
        Self {
            data,
            offset,
            len,
        }
    }
    pub fn get(&self, bit_index: usize) -> Option<bool> {
        let bit_index = self.offset + bit_index;

        if bit_index >= self.offset + self.len { return None }

        let byte_index = bit_index / 8;
        let bits_from_first = bit_index % 8;
        let byte = self.data.get(byte_index)?;
        Some(byte & (1 << bits_from_first) != 0)
    }
    pub fn read_u8(&self, bit_index: usize) -> Option<u8> {
        let bit_index = self.offset + bit_index;

        if bit_index >= self.offset + self.len { return None }

        let first = bit_index / 8;
        let bits_from_first = bit_index % 8;
        let first_byte = self.data.get(first)?;
        let lo = first_byte >> bits_from_first;

        let hi = if bits_from_first > 0 {
            let hi = self.data.get(first + 1)? & ((1 << bits_from_first) - 1);
            let bits_to_next = 8 - bits_from_first;
            hi << bits_to_next
        } else { 0 };


        Some(lo | hi)
    }
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

    let (mut global_state, mut local_state, mut stack) = (GlobalItemsState::default(), LocalItemsState::default(), Vec::new());

    let (_, application_collection, application_global_state, application_local_state) = report_desc.iter().filter_map(|item: &ReportIterItem| 
        match item {
            &ReportIterItem::Item(ref item) => {
                report_desc::update_global_state(&mut global_state, &mut stack, item).unwrap();
                report_desc::update_local_state(&mut local_state, item);
                None
            }
            &ReportIterItem::Collection(n, ref collection) => {
                let lc_state = std::mem::replace(&mut local_state, LocalItemsState::default());
                Some((n, collection, global_state, lc_state))
            }
        }
    ).find(|&(n, _, _, _)| n == MainCollectionFlags::Application as u8).expect("Failed to find application collection");

    // Get all main items, and their global item options.
    {
        let items = application_collection.iter().filter_map(ReportIterItem::as_item).filter_map(|item| match item {
            ReportItem::Global(_) => {
                report_desc::update_global_state(&mut global_state, &mut stack, item).unwrap();
                None
            }
            ReportItem::Main(m) => {
                let lc_state = std::mem::replace(&mut local_state, LocalItemsState::default());
                Some((global_state, lc_state, m))
            }
            ReportItem::Local(_) => {
                report_desc::update_local_state(&mut local_state, item);
                None
            },
        });
        let mut bit_offset = 0;
        let inputs = items.filter_map(|(global_state, local_state, item)| {
            let report_size = match global_state.report_size {
                Some(s) => s,
                None => return None,
            };
            let report_count = match global_state.report_count {
                Some(c) => c,
                None => return None,
            };
            if global_state.usage_page != Some(0x7) {
                return None;
            }
            let bit_length = report_size * report_count;

            let offset = bit_offset;
            bit_offset += bit_length;

            if let &MainItem::Input(flags) = item {
                Some((bit_length, offset, global_state, local_state, MainItemFlags::from_bits_truncate(flags)))
            } else {
                None
            }
        }).collect::<Vec<_>>();
        let total_bit_length = inputs.iter().map(|(bit_length, _, _, _, _)| bit_length).sum();

        let total_byte_length = div_round_up(total_bit_length, 8);

        let mut report_buffer = vec! [0u8; total_byte_length as usize];
        let mut last_buffer = report_buffer.clone();
        let report_ty = ReportTy::Input;
        let report_id = 0;

        let orbital_socket = File::open("display:input").expect("Failed to open orbital input socket");

        let mut pressed_keys = Vec::<u8>::new();
        let mut last_pressed_keys = pressed_keys.clone();

        loop {
            std::thread::sleep(std::time::Duration::from_millis(10));

            std::mem::swap(&mut report_buffer, &mut last_buffer);
            reqs::get_report(&handle, report_ty, report_id, interface_num, &mut report_buffer).expect("Failed to get report");

            if report_buffer == last_buffer {
                continue
            }

            for &(bit_length, bit_offset, global_state, local_state, input) in &inputs {
                let report_size = global_state.report_size.unwrap();
                let report_count = global_state.report_count.unwrap();

                // TODO: For now, the dynamic value usages cannot overlap with selector usages...
                // for now.

                if local_state.usage_min == Some(224) && local_state.usage_max == Some(231) {
                    // The usages that this descriptor references are all dynamic values.
                } else {
                    // The usages are selectors.
                }

                for report_index in 0..report_count {
                }

                /*if input.contains(MainItemFlags::VARIABLE) {
                    // The item is a variable.

                    let binary_view = BinaryView::new(&report_buffer, bit_offset as usize, bit_length as usize);

                    if report_count == 8 && report_size == 1 && local_state.usage_min == Some(224) && local_state.usage_max == Some(231) && global_state.logical_min == Some(0)  && global_state.logical_max == Some(1) {
                    } else {
                        println!("unknown report variable item");
                    }
                } else {
                    // The item is an array.

                    std::mem::swap(&mut pressed_keys, &mut last_pressed_keys);
                    pressed_keys.clear();

                    println!("INPUT FLAGS: {:?}", input);
                    assert_eq!(report_size, 8);
                    for report_index in 0..report_count as usize {
                        let binary_view = BinaryView::new(&report_buffer, bit_offset as usize + report_index * report_size as usize, report_size as usize);
                        let usage = binary_view.read_u8(0).expect("Failed to read array item");
                        if usage != 0 {
                            pressed_keys.push(usage);
                        }
                        println!("Report index array {}: {}", report_index, usage);
                    }
                }*/
            }
            for (current, last) in pressed_keys.iter().copied().zip(last_pressed_keys.iter().copied()) {
                if current == last { continue }
                if current != 0 {
                    // Keycode current changed state to "pressed".
                } else {
                    // Keycode current changed state to "released".
                }
            }
            println!();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn binary_view() {
        // 0000 1000 1100 0111
        //  E             S   
        let view = super::BinaryView::new(&[0xC7, 0x08], 3, 11);
        assert_eq!(view.get(0), Some(false));
        assert_eq!(view.get(2), Some(false));
        assert_eq!(view.get(3), Some(true));
        assert_eq!(view.get(7), Some(false));
        assert_eq!(view.get(17), None);

        assert_eq!(view.read_u8(0), Some(0b0001_1000));
        assert_eq!(view.read_u8(1), Some(0b1000_1100));
        assert_eq!(view.read_u8(2), Some(0b0100_0110));
        assert_eq!(view.read_u8(3), Some(0b0010_0011));
        assert_eq!(view.read_u8(7), None);

        //  0000 1000 1100 0111
        // E        S
        let view = super::BinaryView::new(&[0xC7, 0x08], 8, 8);
        assert_eq!(view.read_u8(0), Some(0b0000_1000));
    }
}
