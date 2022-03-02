use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::str;

use orbclient::{KeyEvent, MouseEvent, MouseRelativeEvent, ButtonEvent, ScrollEvent};

use crate::controller::Ps2;
use crate::vm;

bitflags! {
    pub struct MousePacketFlags: u8 {
        const LEFT_BUTTON = 1;
        const RIGHT_BUTTON = 1 << 1;
        const MIDDLE_BUTTON = 1 << 2;
        const ALWAYS_ON = 1 << 3;
        const X_SIGN = 1 << 4;
        const Y_SIGN = 1 << 5;
        const X_OVERFLOW = 1 << 6;
        const Y_OVERFLOW = 1 << 7;
    }
}

pub struct Ps2d<F: Fn(u8,bool) -> char>  {
    ps2: Ps2,
    vmmouse: bool,
    vmmouse_relative: bool,
    input: File,
    width: u32,
    height: u32,
    lshift: bool,
    rshift: bool,
    mouse_x: i32,
    mouse_y: i32,
    mouse_left: bool,
    mouse_middle: bool,
    mouse_right: bool,
    packets: [u8; 4],
    packet_i: usize,
    extra_packet: bool,
    //Keymap function
    get_char: F
}

impl<F: Fn(u8,bool) -> char> Ps2d<F> {
    pub fn new(input: File, keymap: F) -> Self {
        let mut ps2 = Ps2::new();
        let extra_packet = ps2.init().expect("ps2d: failed to initialize");

        let vmmouse_relative = true;
        let vmmouse = false; //vm::enable(vmmouse_relative);

        let mut ps2d = Ps2d {
            ps2,
            vmmouse,
            vmmouse_relative,
            input,
            width: 0,
            height: 0,
            lshift: false,
            rshift: false,
            mouse_x: 0,
            mouse_y: 0,
            mouse_left: false,
            mouse_middle: false,
            mouse_right: false,
            packets: [0; 4],
            packet_i: 0,
            extra_packet,
            get_char: keymap
        };

        ps2d.resize();

        ps2d
    }

    pub fn resize(&mut self) {
        let mut buf: [u8; 4096] = [0; 4096];
        if let Ok(count) = syscall::fpath(self.input.as_raw_fd() as usize, &mut buf) {
            let path = unsafe { str::from_utf8_unchecked(&buf[..count]) };
            let res = path.split(":").nth(1).unwrap_or("");
            self.width = res.split("/").nth(1).unwrap_or("").parse::<u32>().unwrap_or(0);
            self.height = res.split("/").nth(2).unwrap_or("").parse::<u32>().unwrap_or(0);
        }
    }

    pub fn irq(&mut self) {
        while let Some((keyboard, data)) = self.ps2.next() {
            self.handle(keyboard, data);
        }
    }

    pub fn handle(&mut self, keyboard: bool, data: u8) {
        if keyboard {
            let (scancode, pressed) = if data >= 0x80 {
                (data - 0x80, false)
            } else {
                (data, true)
            };

            if scancode == 0x2A {
                self.lshift = pressed;
            } else if scancode == 0x36 {
                self.rshift = pressed;
            }

            self.input.write(&KeyEvent {
                character: (self.get_char)(scancode, self.lshift || self.rshift),
                scancode: scancode,
                pressed: pressed
            }.to_event()).expect("ps2d: failed to write key event");
        } else if self.vmmouse {
            for _i in 0..256 {
        		let (status, _, _, _, _, _) = unsafe { vm::cmd(vm::ABSPOINTER_STATUS, 0) };
        		//TODO if ((status & VMMOUSE_ERROR) == VMMOUSE_ERROR)

        		let queue_length = status & 0xffff;
        		if queue_length == 0 {
        			break;
                }

        		if queue_length % 4 != 0 {
        			eprintln!("ps2d: queue length not a multiple of 4: {}", queue_length);
        			break;
        		}

        		let (status, dx, dy, dz, _, _) = unsafe { vm::cmd(vm::ABSPOINTER_DATA, 4) };

                if self.vmmouse_relative {
                    if dx != 0 || dy != 0 {
                        self.input.write(&MouseRelativeEvent {
                            dx: dx as i32,
                            dy: dy as i32,
                        }.to_event()).expect("ps2d: failed to write mouse event");
                    }
        		} else {
                    // TODO: Improve efficiency
                    self.resize();

                    let x = dx as i32 * self.width as i32 / 0xFFFF;
                    let y = dy as i32 * self.height as i32 / 0xFFFF;
                    if x != self.mouse_x || y != self.mouse_y {
                        self.mouse_x = x;
                        self.mouse_y = y;
                        self.input.write(&MouseEvent {
                            x: x,
                            y: y,
                        }.to_event()).expect("ps2d: failed to write mouse event");
                    }
        		};

                if dz != 0 {
                    self.input.write(&ScrollEvent {
                        x: 0,
                        y: -(dz as i32),
                    }.to_event()).expect("ps2d: failed to write scroll event");
                }

                let left = status & vm::LEFT_BUTTON == vm::LEFT_BUTTON;
                let middle = status & vm::MIDDLE_BUTTON == vm::MIDDLE_BUTTON;
                let right = status & vm::RIGHT_BUTTON == vm::RIGHT_BUTTON;
                if left != self.mouse_left || middle != self.mouse_middle || right != self.mouse_right {
                    self.mouse_left = left;
                    self.mouse_middle = middle;
                    self.mouse_right = right;
                    self.input.write(&ButtonEvent {
                        left: left,
                        middle: middle,
                        right: right,
                    }.to_event()).expect("ps2d: failed to write button event");
                }
            }
        } else {
            self.packets[self.packet_i] = data;
            self.packet_i += 1;

            let flags = MousePacketFlags::from_bits_truncate(self.packets[0]);
            if ! flags.contains(MousePacketFlags::ALWAYS_ON) {
                eprintln!("ps2d: mouse misalign {:X}", self.packets[0]);

                self.packets = [0; 4];
                self.packet_i = 0;
            } else if self.packet_i >= self.packets.len() || (!self.extra_packet && self.packet_i >= 3) {
                if ! flags.contains(MousePacketFlags::X_OVERFLOW) && ! flags.contains(MousePacketFlags::Y_OVERFLOW) {
                    let mut dx = self.packets[1] as i32;
                    if flags.contains(MousePacketFlags::X_SIGN) {
                        dx -= 0x100;
                    }

                    let mut dy = -(self.packets[2] as i32);
                    if flags.contains(MousePacketFlags::Y_SIGN) {
                        dy += 0x100;
                    }

                    let mut dz = 0;
                    if self.extra_packet {
                        let mut scroll = (self.packets[3] & 0xF) as i8;
                        if scroll & (1 << 3) == 1 << 3 {
                            scroll -= 16;
                        }
                        dz = -scroll as i32;
                    }

                    if dx != 0 || dy != 0 {
                        self.input.write(&MouseRelativeEvent {
                            dx: dx,
                            dy: dy,
                        }.to_event()).expect("ps2d: failed to write mouse event");
                    }

                    if dz != 0 {
                        self.input.write(&ScrollEvent {
                            x: 0,
                            y: dz,
                        }.to_event()).expect("ps2d: failed to write scroll event");
                    }

                    let left = flags.contains(MousePacketFlags::LEFT_BUTTON);
                    let middle = flags.contains(MousePacketFlags::MIDDLE_BUTTON);
                    let right = flags.contains(MousePacketFlags::RIGHT_BUTTON);
                    if left != self.mouse_left || middle != self.mouse_middle || right != self.mouse_right {
                        self.mouse_left = left;
                        self.mouse_middle = middle;
                        self.mouse_right = right;
                        self.input.write(&ButtonEvent {
                            left: left,
                            middle: middle,
                            right: right,
                        }.to_event()).expect("ps2d: failed to write button event");
                    }
                } else {
                    eprintln!("ps2d: overflow {:X} {:X} {:X} {:X}", self.packets[0], self.packets[1], self.packets[2], self.packets[3]);
                }

                self.packets = [0; 4];
                self.packet_i = 0;
            }
        }
    }
}
