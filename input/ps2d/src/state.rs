use inputd::ProducerHandle;
use log::{error, warn};
use orbclient::{ButtonEvent, KeyEvent, MouseEvent, MouseRelativeEvent, ScrollEvent};

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

pub struct Ps2d<F: Fn(u8, bool) -> char> {
    ps2: Ps2,
    vmmouse: bool,
    vmmouse_relative: bool,
    input: ProducerHandle,
    extended: bool,
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
    get_char: F,
}

impl<F: Fn(u8, bool) -> char> Ps2d<F> {
    pub fn new(input: ProducerHandle, keymap: F) -> Self {
        let mut ps2 = Ps2::new();
        let extra_packet = ps2.init();

        // FIXME add an option for orbital to disable this when an app captures the mouse.
        let vmmouse_relative = false;
        let vmmouse = vm::enable(vmmouse_relative);

        Ps2d {
            ps2,
            vmmouse,
            vmmouse_relative,
            input,
            extended: false,
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
            get_char: keymap,
        }
    }

    pub fn update_keymap(&mut self, keymap: F) {
        self.get_char = keymap;
    }

    pub fn irq(&mut self) {
        while let Some((keyboard, data)) = self.ps2.next() {
            self.handle(keyboard, data);
        }
    }

    pub fn handle(&mut self, keyboard: bool, data: u8) {
        if keyboard {
            if data == 0xE0 {
                self.extended = true;
            } else {
                let (ps2_scancode, pressed) = if data >= 0x80 {
                    (data - 0x80, false)
                } else {
                    (data, true)
                };

                let scancode = if self.extended {
                    self.extended = false;
                    match ps2_scancode {
                        //TODO: media keys
                        //TODO: 0x1C => orbclient::K_NUM_ENTER,
                        0x1D => orbclient::K_CTRL, //TODO: 0x1D => orbclient::K_RIGHT_CTRL,
                        0x20 => 0x80 + 0x20,       //TODO: orbclient::K_VOLUME_MUTE,
                        0x2E => 0x80 + 0x2E,       //TODO: orbclient::K_VOLUME_DOWN,
                        0x30 => 0x80 + 0x30,       //TODO: orbclient::K_VOLUME_UP,
                        //TODO: 0x35 => orbclient::K_NUM_SLASH,
                        0x38 => orbclient::K_ALT_GR,
                        0x47 => orbclient::K_HOME,
                        0x48 => orbclient::K_UP,
                        0x49 => orbclient::K_PGUP,
                        0x4B => orbclient::K_LEFT,
                        0x4D => orbclient::K_RIGHT,
                        0x4F => orbclient::K_END,
                        0x50 => orbclient::K_DOWN,
                        0x51 => orbclient::K_PGDN,
                        //TODO: 0x52 => orbclient::K_INSERT,
                        0x53 => orbclient::K_DEL,
                        0x5B => 0x5B, //TODO: orbclient::K_LEFT_SUPER,
                        //TODO: 0x5C => orbclient::K_RIGHT_SUPER,
                        //TODO: 0x5D => orbclient::K_APP,
                        //TODO power keys
                        /* 0x80 to 0xFF used for press/release detection */
                        _ => {
                            if pressed {
                                warn!("ps2d: unknown extended scancode {:02X}", ps2_scancode);
                            }
                            0
                        }
                    }
                } else {
                    match ps2_scancode {
                        /* 0x00 unused */
                        0x01 => orbclient::K_ESC,
                        0x02 => orbclient::K_1,
                        0x03 => orbclient::K_2,
                        0x04 => orbclient::K_3,
                        0x05 => orbclient::K_4,
                        0x06 => orbclient::K_5,
                        0x07 => orbclient::K_6,
                        0x08 => orbclient::K_7,
                        0x09 => orbclient::K_8,
                        0x0A => orbclient::K_9,
                        0x0B => orbclient::K_0,
                        0x0C => orbclient::K_MINUS,
                        0x0D => orbclient::K_EQUALS,
                        0x0E => orbclient::K_BKSP,
                        0x0F => orbclient::K_TAB,
                        0x10 => orbclient::K_Q,
                        0x11 => orbclient::K_W,
                        0x12 => orbclient::K_E,
                        0x13 => orbclient::K_R,
                        0x14 => orbclient::K_T,
                        0x15 => orbclient::K_Y,
                        0x16 => orbclient::K_U,
                        0x17 => orbclient::K_I,
                        0x18 => orbclient::K_O,
                        0x19 => orbclient::K_P,
                        0x1A => orbclient::K_BRACE_OPEN,
                        0x1B => orbclient::K_BRACE_CLOSE,
                        0x1C => orbclient::K_ENTER,
                        0x1D => orbclient::K_CTRL,
                        0x1E => orbclient::K_A,
                        0x1F => orbclient::K_S,
                        0x20 => orbclient::K_D,
                        0x21 => orbclient::K_F,
                        0x22 => orbclient::K_G,
                        0x23 => orbclient::K_H,
                        0x24 => orbclient::K_J,
                        0x25 => orbclient::K_K,
                        0x26 => orbclient::K_L,
                        0x27 => orbclient::K_SEMICOLON,
                        0x28 => orbclient::K_QUOTE,
                        0x29 => orbclient::K_TICK,
                        0x2A => orbclient::K_LEFT_SHIFT,
                        0x2B => orbclient::K_BACKSLASH,
                        0x2C => orbclient::K_Z,
                        0x2D => orbclient::K_X,
                        0x2E => orbclient::K_C,
                        0x2F => orbclient::K_V,
                        0x30 => orbclient::K_B,
                        0x31 => orbclient::K_N,
                        0x32 => orbclient::K_M,
                        0x33 => orbclient::K_COMMA,
                        0x34 => orbclient::K_PERIOD,
                        0x35 => orbclient::K_SLASH,
                        0x36 => orbclient::K_RIGHT_SHIFT,
                        //TODO: 0x37 => orbclient::K_NUM_ASTERISK,
                        0x38 => orbclient::K_ALT,
                        0x39 => orbclient::K_SPACE,
                        0x3A => orbclient::K_CAPS,
                        0x3B => orbclient::K_F1,
                        0x3C => orbclient::K_F2,
                        0x3D => orbclient::K_F3,
                        0x3E => orbclient::K_F4,
                        0x3F => orbclient::K_F5,
                        0x40 => orbclient::K_F6,
                        0x41 => orbclient::K_F7,
                        0x42 => orbclient::K_F8,
                        0x43 => orbclient::K_F9,
                        0x44 => orbclient::K_F10,
                        //TODO: 0x45 => orbclient::K_NUM_LOCK,
                        //TODO: 0x46 => orbclient::K_SCROLL_LOCK,
                        0x47 => orbclient::K_NUM_7,
                        0x48 => orbclient::K_NUM_8,
                        0x49 => orbclient::K_NUM_9,
                        //TODO: 0x4A => orbclient::K_NUM_MINUS,
                        0x4B => orbclient::K_NUM_4,
                        0x4C => orbclient::K_NUM_5,
                        0x4D => orbclient::K_NUM_6,
                        //TODO: 0x4E => orbclient::K_NUM_PLUS,
                        0x4F => orbclient::K_NUM_1,
                        0x50 => orbclient::K_NUM_2,
                        0x51 => orbclient::K_NUM_3,
                        0x52 => orbclient::K_NUM_0,
                        //TODO: 0x53 => orbclient::K_NUM_PERIOD,
                        /* 0x54 to 0x56 unused */
                        0x57 => orbclient::K_F11,
                        0x58 => orbclient::K_F12,
                        /* 0x59 to 0x7F unused */
                        /* 0x80 to 0xFF used for press/release detection */
                        _ => {
                            if pressed {
                                warn!("ps2d: unknown scancode {:02X}", ps2_scancode);
                            }
                            0
                        }
                    }
                };

                if scancode == orbclient::K_LEFT_SHIFT {
                    self.lshift = pressed;
                } else if scancode == orbclient::K_RIGHT_SHIFT {
                    self.rshift = pressed;
                }

                if scancode != 0 {
                    self.input
                        .write_event(
                            KeyEvent {
                                character: (self.get_char)(
                                    ps2_scancode,
                                    self.lshift || self.rshift,
                                ),
                                scancode,
                                pressed,
                            }
                            .to_event(),
                        )
                        .expect("ps2d: failed to write key event");
                }
            }
        } else if self.vmmouse {
            for _i in 0..256 {
                let (status, _, _, _) = unsafe { vm::cmd(vm::ABSPOINTER_STATUS, 0) };
                //TODO if ((status & VMMOUSE_ERROR) == VMMOUSE_ERROR)

                let queue_length = status & 0xffff;
                if queue_length == 0 {
                    break;
                }

                if queue_length % 4 != 0 {
                    error!("ps2d: queue length not a multiple of 4: {}", queue_length);
                    break;
                }

                let (status, dx, dy, dz) = unsafe { vm::cmd(vm::ABSPOINTER_DATA, 4) };

                if self.vmmouse_relative {
                    if dx != 0 || dy != 0 {
                        self.input
                            .write_event(
                                MouseRelativeEvent {
                                    dx: dx as i32,
                                    dy: dy as i32,
                                }
                                .to_event(),
                            )
                            .expect("ps2d: failed to write mouse event");
                    }
                } else {
                    let x = dx as i32;
                    let y = dy as i32;
                    if x != self.mouse_x || y != self.mouse_y {
                        self.mouse_x = x;
                        self.mouse_y = y;
                        self.input
                            .write_event(MouseEvent { x, y }.to_event())
                            .expect("ps2d: failed to write mouse event");
                    }
                };

                if dz != 0 {
                    self.input
                        .write_event(
                            ScrollEvent {
                                x: 0,
                                y: -(dz as i32),
                            }
                            .to_event(),
                        )
                        .expect("ps2d: failed to write scroll event");
                }

                let left = status & vm::LEFT_BUTTON == vm::LEFT_BUTTON;
                let middle = status & vm::MIDDLE_BUTTON == vm::MIDDLE_BUTTON;
                let right = status & vm::RIGHT_BUTTON == vm::RIGHT_BUTTON;
                if left != self.mouse_left
                    || middle != self.mouse_middle
                    || right != self.mouse_right
                {
                    self.mouse_left = left;
                    self.mouse_middle = middle;
                    self.mouse_right = right;
                    self.input
                        .write_event(
                            ButtonEvent {
                                left,
                                middle,
                                right,
                            }
                            .to_event(),
                        )
                        .expect("ps2d: failed to write button event");
                }
            }
        } else {
            self.packets[self.packet_i] = data;
            self.packet_i += 1;

            let flags = MousePacketFlags::from_bits_truncate(self.packets[0]);
            if !flags.contains(MousePacketFlags::ALWAYS_ON) {
                error!("ps2d: mouse misalign {:X}", self.packets[0]);

                self.packets = [0; 4];
                self.packet_i = 0;
            } else if self.packet_i >= self.packets.len()
                || (!self.extra_packet && self.packet_i >= 3)
            {
                if !flags.contains(MousePacketFlags::X_OVERFLOW)
                    && !flags.contains(MousePacketFlags::Y_OVERFLOW)
                {
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
                        self.input
                            .write_event(MouseRelativeEvent { dx, dy }.to_event())
                            .expect("ps2d: failed to write mouse event");
                    }

                    if dz != 0 {
                        self.input
                            .write_event(ScrollEvent { x: 0, y: dz }.to_event())
                            .expect("ps2d: failed to write scroll event");
                    }

                    let left = flags.contains(MousePacketFlags::LEFT_BUTTON);
                    let middle = flags.contains(MousePacketFlags::MIDDLE_BUTTON);
                    let right = flags.contains(MousePacketFlags::RIGHT_BUTTON);
                    if left != self.mouse_left
                        || middle != self.mouse_middle
                        || right != self.mouse_right
                    {
                        self.mouse_left = left;
                        self.mouse_middle = middle;
                        self.mouse_right = right;
                        self.input
                            .write_event(
                                ButtonEvent {
                                    left,
                                    middle,
                                    right,
                                }
                                .to_event(),
                            )
                            .expect("ps2d: failed to write button event");
                    }
                } else {
                    warn!(
                        "ps2d: overflow {:X} {:X} {:X} {:X}",
                        self.packets[0], self.packets[1], self.packets[2], self.packets[3]
                    );
                }

                self.packets = [0; 4];
                self.packet_i = 0;
            }
        }
    }
}
