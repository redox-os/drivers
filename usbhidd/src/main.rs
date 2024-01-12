use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;

use bitflags::bitflags;
use orbclient::KeyEvent as OrbKeyEvent;
use redox_log::{OutputBuilder, RedoxLogger};
use xhcid_interface::{ConfigureEndpointsReq, DevDesc, PortReqRecipient, XhciClientHandle};

mod keymap;
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

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "device", "hid.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create hid.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "device", "hid.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create hid.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("usbhidd: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("usbhidd: failed to set default logger: {}", error);
            None
        }
    }
}

fn send_key_event(display: &mut File, usage_page: u32, usage: u8, pressed: bool, shift_opt: Option<bool>) {
    let scancode = match usage_page {
        0x07 => match usage {
            0x04 => orbclient::K_A,
            0x05 => orbclient::K_B,
            0x06 => orbclient::K_C,
            0x07 => orbclient::K_D,
            0x08 => orbclient::K_E,
            0x09 => orbclient::K_F,
            0x0A => orbclient::K_G,
            0x0B => orbclient::K_H,
            0x0C => orbclient::K_I,
            0x0D => orbclient::K_J,
            0x0E => orbclient::K_K,
            0x0F => orbclient::K_L,
            0x10 => orbclient::K_M,
            0x11 => orbclient::K_N,
            0x12 => orbclient::K_O,
            0x13 => orbclient::K_P,
            0x14 => orbclient::K_Q,
            0x15 => orbclient::K_R,
            0x16 => orbclient::K_S,
            0x17 => orbclient::K_T,
            0x18 => orbclient::K_U,
            0x19 => orbclient::K_V,
            0x1A => orbclient::K_W,
            0x1B => orbclient::K_X,
            0x1C => orbclient::K_Y,
            0x1D => orbclient::K_Z,
            0x1E => orbclient::K_1,
            0x1F => orbclient::K_2,
            0x20 => orbclient::K_3,
            0x21 => orbclient::K_4,
            0x22 => orbclient::K_5,
            0x23 => orbclient::K_6,
            0x24 => orbclient::K_7,
            0x25 => orbclient::K_8,
            0x26 => orbclient::K_9,
            0x27 => orbclient::K_0,
            0x28 => orbclient::K_ENTER,
            0x29 => orbclient::K_ESC,
            0x2A => orbclient::K_BKSP,
            0x2B => orbclient::K_TAB,
            0x2C => orbclient::K_SPACE,
            0x2D => orbclient::K_MINUS,
            0x2E => orbclient::K_EQUALS,
            0x2F => orbclient::K_BRACE_OPEN,
            0x30 => orbclient::K_BRACE_CLOSE,
            0x31 => orbclient::K_BACKSLASH,
            // 0x32 non-us # and ~
            0x33 => orbclient::K_SEMICOLON,
            0x34 => orbclient::K_QUOTE,
            0x35 => orbclient::K_TICK,
            0x36 => orbclient::K_COMMA,
            0x37 => orbclient::K_PERIOD,
            0x38 => orbclient::K_SLASH,
            0x39 => orbclient::K_CAPS,
            0x3A => orbclient::K_F1,
            0x3B => orbclient::K_F2,
            0x3C => orbclient::K_F3,
            0x3D => orbclient::K_F4,
            0x3E => orbclient::K_F5,
            0x3F => orbclient::K_F6,
            0x40 => orbclient::K_F7,
            0x41 => orbclient::K_F8,
            0x42 => orbclient::K_F9,
            0x43 => orbclient::K_F10,
            0x44 => orbclient::K_F11,
            0x45 => orbclient::K_F12,
            // 0x46 print screen
            // 0x47 scroll lock
            // 0x48 pause
            // 0x49 insert
            0x4A => orbclient::K_HOME,
            0x4B => orbclient::K_PGUP,
            0x4C => orbclient::K_DEL,
            0x4D => orbclient::K_END,
            0x4E => orbclient::K_PGDN,
            0x4F => orbclient::K_RIGHT,
            0x50 => orbclient::K_LEFT,
            0x51 => orbclient::K_DOWN,
            0x52 => orbclient::K_UP,
            // 0x53 num lock
            // 0x54 num /
            // 0x55 num *
            // 0x56 num -
            // 0x57 num +
            // 0x58 num enter
            0x59 => orbclient::K_NUM_1,
            0x5A => orbclient::K_NUM_2,
            0x5B => orbclient::K_NUM_3,
            0x5C => orbclient::K_NUM_4,
            0x5D => orbclient::K_NUM_5,
            0x5E => orbclient::K_NUM_6,
            0x5F => orbclient::K_NUM_7,
            0x60 => orbclient::K_NUM_8,
            0x61 => orbclient::K_NUM_9,
            0x62 => orbclient::K_NUM_0,
            // 0x62 num .
            // 0x64 non-us \ and |
            // 0x64 app
            // 0x66 power
            // 0x67 num =
            // unmapped values
            0xE0 => orbclient::K_CTRL, // TODO: left control
            0xE1 => orbclient::K_LEFT_SHIFT,
            0xE2 => orbclient::K_ALT,
            0xE3 => 0x5B, // left super
            0xE4 => orbclient::K_CTRL, // TODO: right control
            0xE5 => orbclient::K_RIGHT_SHIFT,
            0xE6 => orbclient::K_ALT_GR,
            // 0xE7 right super
            // reserved values
            _ => {
                log::warn!("unknown usage_page {:#x} usage {:#x}", usage_page, usage);
                return;
            },
        },
        _ => {
            log::warn!("unknown usage_page {:#x}", usage_page);
            return;
        },
    };

    //TODO: other keymaps
    let character = if let Some(shift) = shift_opt {
        keymap::us::get_char(scancode, shift)
    } else {
        '\0'
    };

    let key_event = OrbKeyEvent {
        character,
        scancode,
        pressed,
    };

    match display.write(&key_event.to_event()) {
        Ok(_) => (),
        Err(err) => {
            log::warn!("failed to send key event to orbital: {}", err);
        }
    }
}

fn main() {
    let _logger_ref = setup_logging();

    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbhidd <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("Expected integer as input of port");
    let protocol = args.next().expect(USAGE);

    log::info!(
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
        log::debug!("{:?}", item);
    }

    handle.configure_endpoints(&ConfigureEndpointsReq { config_desc: 0, interface_desc: None, alternate_setting: None }).expect("Failed to configure endpoints");

    let (mut global_state, mut local_state, mut stack) = (GlobalItemsState::default(), LocalItemsState::default(), Vec::new());

    let (_, application_collection, application_global_state, application_local_state) = report_desc.iter().filter_map(|item: &ReportIterItem| {
        log::trace!("1: {:?}", item);
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
    }).find(|&(n, _, _, _)| n == MainCollectionFlags::Application as u8).expect("Failed to find application collection");

    // Get all main items, and their global item options.
    {
        let mut collections = VecDeque::new();
        collections.push_back(application_collection);
        let mut items = Vec::new();
        while let Some(collection) = collections.pop_front() {
            for item in collection {
                log::trace!("2: {:?}", item);
                match item {
                    ReportIterItem::Item(item) => match item {
                        ReportItem::Global(_) => {
                            report_desc::update_global_state(&mut global_state, &mut stack, item).unwrap();
                        }
                        ReportItem::Main(m) => {
                            let lc_state = std::mem::replace(&mut local_state, LocalItemsState::default());
                            items.push((global_state, lc_state, m));
                        }
                        ReportItem::Local(_) => {
                            report_desc::update_local_state(&mut local_state, item);
                        },
                    },
                    //TODO: does local state need to be different for inner collections?
                    ReportIterItem::Collection(_, collection) => {
                        collections.push_back(collection);
                    },
                }
            }
        }
        let mut bit_offset = 0;
        let inputs = items.iter().filter_map(|(global_state, local_state, item)| {
            log::trace!("3: {:?}", item);

            if let &MainItem::Input(flags) = item {
                let report_size = match global_state.report_size {
                    Some(s) => s,
                    None => return None,
                };
                let report_count = match global_state.report_count {
                    Some(c) => c,
                    None => return None,
                };

                let bit_length = report_size * report_count;
                let offset = bit_offset;
                bit_offset += bit_length;

                Some((bit_length, offset, global_state, local_state, MainItemFlags::from_bits_truncate(*flags)))
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

        let mut display = File::open("input:producer").expect("Failed to open orbital input socket");

        let mut pressed_keys = Vec::<(u32, u8)>::new();
        let mut last_pressed_keys = pressed_keys.clone();
        let mut last_buttons = (false, false, false);

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

                log::trace!(
                    "size {} count {} at {} length {}",
                    report_size,
                    report_count,
                    bit_offset,
                    bit_length
                );

                // TODO: For now, the dynamic value usages cannot overlap with selector usages...
                // for now.

                if local_state.usage_min == Some(224) && local_state.usage_max == Some(231) {
                    // The usages that this descriptor references are all dynamic values.
                } else {
                    // The usages are selectors.
                }

                if input.contains(MainItemFlags::VARIABLE) {
                    // The item is a variable.

                    let binary_view = BinaryView::new(&report_buffer, bit_offset as usize, bit_length as usize);

                    if report_count == 8 && report_size == 1 && global_state.usage_page == Some(7) && local_state.usage_min == Some(224) && local_state.usage_max == Some(231) && global_state.logical_min == Some(0)  && global_state.logical_max == Some(1) {
                        let bits = binary_view.read_u8(0).expect("Failed to read array item");
                        for bit in 0..8 {
                            if bits & (1 << bit) > 0 {
                                pressed_keys.push((0x07, 0xE0 + bit));
                            }
                        }
                        log::trace!("Report variable {:#x?}", bits);
                    } else if report_count == 2 && report_size == 16 && global_state.usage_page == Some(1) {
                        //TODO: Make this less hard-coded
                        let raw_x =
                            binary_view.read_u8(0).expect("Failed to read array item") as u16 |
                            (binary_view.read_u8(8).expect("Failed to read array item") as u16) << 8;
                        let raw_y =
                            binary_view.read_u8(16).expect("Failed to read array item") as u16 |
                            (binary_view.read_u8(24).expect("Failed to read array item") as u16) << 8;

                        // ps2d uses 0..=65535 as range, while usb uses 0..=32767. orbital
                        // expects the former range, so multiply by two here to translate
                        // the usb coordinates to what orbital expects.
                        let x = raw_x * 2;
                        let y = raw_y * 2;

                        log::trace!("Touchscreen {}, {} => {}, {}", raw_x, raw_y, x, y);
                        if x != 0 || y != 0 {
                            let mouse_event = orbclient::event::MouseEvent {
                                x: x as i32,
                                y: y as i32,
                            };

                            match display.write(&mouse_event.to_event()) {
                                Ok(_) => (),
                                Err(err) => {
                                    log::warn!("failed to send mouse event to orbital: {}", err);
                                }
                            }
                        }
                    } else if report_count == 3 && report_size == 8 && global_state.usage_page == Some(1) {
                        //TODO: Make this less hard-coded
                        let dx = binary_view.read_u8(0).expect("Failed to read array item") as i8;
                        let dy = binary_view.read_u8(8).expect("Failed to read array item") as i8;
                        let dz = binary_view.read_u8(16).expect("Failed to read array item") as i8;
                        log::trace!("Mouse {}, {}, {}", dx, dy, dz);
                        if dx != 0 || dy != 0 {
                            let mouse_event = orbclient::event::MouseRelativeEvent {
                                dx: dx as i32,
                                dy: dy as i32,
                            };

                            match display.write(&mouse_event.to_event()) {
                                Ok(_) => (),
                                Err(err) => {
                                    log::warn!("failed to send mouse event to orbital: {}", err);
                                }
                            }
                        }
                        if dz != 0 {
                            let scroll_event = orbclient::event::ScrollEvent {
                                x: dz as i32,
                                y: 0,
                            };

                            match display.write(&scroll_event.to_event()) {
                                Ok(_) => (),
                                Err(err) => {
                                    log::warn!("failed to send scroll event to orbital: {}", err);
                                }
                            }
                        }
                    } else if report_count == 3 && report_size == 1 && global_state.usage_page == Some(9) {
                        //TODO: Make this less hard-coded
                        let left = binary_view.get(0).expect("Failed to read array item");
                        let right = binary_view.get(1).expect("Failed to read array item");
                        let middle = binary_view.get(2).expect("Failed to read array item");
                        log::trace!("Left {}, Right {}, Middle {}", left, right, middle);
                        if last_buttons != (left, right, middle) {
                            last_buttons = (left, right, middle);

                            let button_event = orbclient::event::ButtonEvent {
                                left,
                                right,
                                middle,
                            };

                            match display.write(&button_event.to_event()) {
                                Ok(_) => (),
                                Err(err) => {
                                    log::warn!("failed to send button event to orbital: {}", err);
                                }
                            }
                        }
                    } else {
                        log::trace!("Unknown report variable item: size {} count {} at {}", report_size, report_count, bit_offset);
                    }
                } else {
                    // The item is an array.

                    log::trace!("INPUT FLAGS: {:?}", input);
                    if report_size == 8 {
                        for report_index in 0..report_count as usize {
                            let binary_view = BinaryView::new(&report_buffer, bit_offset as usize + report_index * report_size as usize, report_size as usize);
                            let usage = binary_view.read_u8(0).expect("Failed to read array item");
                            if usage != 0 {
                                pressed_keys.push((global_state.usage_page.unwrap_or(0), usage));
                            }
                            log::trace!("Report index array {}: {:#x}", report_index, usage);
                        }
                    } else {
                        log::trace!("Unknown report array item: size {} count {} at {}", report_size, report_count, bit_offset);
                    }
                }
            }


            for &(usage_page, usage) in last_pressed_keys.iter() {
                if ! pressed_keys.contains(&(usage_page, usage)) {
                    log::debug!("Released {:#x},{:#x}", usage_page, usage);
                    send_key_event(&mut display, usage_page, usage, false, None);
                }
            }

            for &(usage_page, usage) in pressed_keys.iter() {
                if ! last_pressed_keys.contains(&(usage_page, usage)) {
                    log::debug!("Pressed {:#x},{:#x}", usage_page, usage);
                    send_key_event(&mut display, usage_page, usage, true, Some(
                        pressed_keys.contains(&(0x07, 0xE1)) || pressed_keys.contains(&(0x07, 0xE5))
                    ));
                }
            }

            std::mem::swap(&mut pressed_keys, &mut last_pressed_keys);
            pressed_keys.clear();
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
