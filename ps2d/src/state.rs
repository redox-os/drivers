use orbclient::{KeyEvent, MouseEvent, ButtonEvent, ScrollEvent, K_ALT_GR, K_NUM_0, K_NUM_1, K_NUM_4, K_NUM_7};
use std::{u8, cmp};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::convert::TryFrom;

use keymap::Keymap;
use byteorder::{BigEndian, ByteOrder};

use controller::Ps2;
use vm;

use std::default::Default;
use std::fs::File;
use syscall;
use syscall::{Result, Error, EBADF, EINVAL, ENOENT, SchemeMut};
use syscall::flag::{SEEK_SET, SEEK_CUR, SEEK_END};

use ::keymap::{N_MOD_COMBOS, MAX_KEYCODE};

bitflags! {
    flags MousePacketFlags: u8 {
        const LEFT_BUTTON = 1,
        const RIGHT_BUTTON = 1 << 1,
        const MIDDLE_BUTTON = 1 << 2,
        const ALWAYS_ON = 1 << 3,
        const X_SIGN = 1 << 4,
        const Y_SIGN = 1 << 5,
        const X_OVERFLOW = 1 << 6,
        const Y_OVERFLOW = 1 << 7
    }
}

// Some scancodes (whatever is useful in the Ps2d impl)
const SC_ALT: u8 = 0x38;
const SC_NUM_7: u8 = 0x47;
const SC_NUM_9: u8 = 0x49;
const SC_NUM_4: u8 = 0x4b;
const SC_NUM_6: u8 = 0x4d;
const SC_NUM_1: u8 = 0x4f;
const SC_NUM_3: u8 = 0x51;
const SC_NUM_0: u8 = 0x52;

pub struct Ps2d  {
    ps2: Ps2,
    vmmouse: bool,
    input: File,
    width: u32,
    height: u32,
    lshift: bool,
    rshift: bool,
    alt_gr: bool,
    mouse_x: i32,
    mouse_y: i32,
    mouse_left: bool,
    mouse_middle: bool,
    mouse_right: bool,
    packets: [u8; 4],
    packet_i: usize,
    extra_packet: bool,
    keymap: Keymap,
    // state to switch to PS/2 Scan Code Set 2
    scancode_set_2: bool,
    num_lock: bool,

    // Scheme (at the moment only keymap)
    open: bool,
    pos: usize,
}

impl Ps2d {
    pub fn new(input: File) -> Self {
        let mut ps2 = Ps2::new();
        let extra_packet = ps2.init();

        let vmmouse = vm::enable();

        let mut ps2d = Ps2d {
            ps2: ps2,
            vmmouse: vmmouse,
            input: input,
            width: 0,
            height: 0,
            lshift: false,
            rshift: false,
            alt_gr: false,
            mouse_x: 0,
            mouse_y: 0,
            mouse_left: false,
            mouse_middle: false,
            mouse_right: false,
            packets: [0; 4],
            packet_i: 0,
            extra_packet: extra_packet,
            keymap: Keymap::default(),
            scancode_set_2: false,
            num_lock: true,

            open: false,
            pos: 0,
        };

        ps2d.resize();

        ps2d
    }

    pub fn resize(&mut self) {
        let mut buf: [u8; 4096] = [0; 4096];
        if let Ok(count) = syscall::fpath(self.input.as_raw_fd() as usize, &mut buf) {
            let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
            let res = path.split(":").nth(1).unwrap_or("");
            self.width = res.split("/").nth(1).unwrap_or("").parse::<u32>().unwrap_or(0);
            self.height = res.split("/").nth(2).unwrap_or("").parse::<u32>().unwrap_or(0);
        }
    }

    pub fn irq(&mut self) {
        while let Some((keyboard, data)) = self.ps2.next() {
            self.handle_input(keyboard, data);
        }
    }

    fn handle_input(&mut self, keyboard: bool, data: u8) {
        // TODO: Improve efficiency
        self.resize();

        if keyboard {
            if data == 0xE0 {
                self.scancode_set_2 = true;
            } else {
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

                let keycode = if self.scancode_set_2 {
                    self.scancode_set_2 = false;
                    if scancode == SC_ALT {
                        self.alt_gr = pressed;
                        K_ALT_GR
                    } else {
                        scancode
                    }
                } else {
                    // Num Lock => convert numpad to numbers, ELSE passthrough
                    if self.num_lock {
                        match scancode {
                            c @ SC_NUM_7...SC_NUM_9 => c - SC_NUM_7 + K_NUM_7,
                            c @ SC_NUM_4...SC_NUM_6 => c - SC_NUM_4 + K_NUM_4,
                            c @ SC_NUM_1...SC_NUM_3 => c - SC_NUM_1 + K_NUM_1,
                            SC_NUM_0 => K_NUM_0,
                            _ => scancode,
                        }
                    } else {
                        scancode
                    }
                };

                self.input.write(&KeyEvent {
                    character: self.keymap.get_char(keycode, self.lshift || self.rshift, self.alt_gr),
                    scancode: keycode,
                    pressed: pressed
                }.to_event()).expect("ps2d: failed to write key event");
            }
        } else if self.vmmouse {
            for _i in 0..256 {
        		let (status, _, _, _, _, _) = unsafe { vm::cmd(vm::ABSPOINTER_STATUS, 0) };
        		//TODO if ((status & VMMOUSE_ERROR) == VMMOUSE_ERROR)

        		let queue_length = status & 0xffff;
        		if queue_length == 0 {
        			break;
                }

        		if queue_length % 4 != 0 {
        			println!("queue length not a multiple of 4: {}", queue_length);
        			break;
        		}

        		let (status, dx, dy, dz, _, _) = unsafe { vm::cmd(vm::ABSPOINTER_DATA, 4) };

                let (x, y) = if status & vm::RELATIVE_PACKET == vm::RELATIVE_PACKET {
                    (
                        cmp::max(0, cmp::min(self.width as i32, self.mouse_x + dx as i32)),
                        cmp::max(0, cmp::min(self.height as i32, self.mouse_y - dy as i32))
                    )
        		} else {
                    (
                        dx as i32 * self.width as i32 / 0xFFFF,
                        dy as i32 * self.height as i32 / 0xFFFF
                    )
        		};

                if x != self.mouse_x || y != self.mouse_y {
                    self.mouse_x = x;
                    self.mouse_y = y;
                    self.input.write(&MouseEvent {
                        x: x,
                        y: y,
                    }.to_event()).expect("ps2d: failed to write mouse event");
                }

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
            if ! flags.contains(ALWAYS_ON) {
                println!("MOUSE MISALIGN {:X}", self.packets[0]);

                self.packets = [0; 4];
                self.packet_i = 0;
            } else if self.packet_i >= self.packets.len() || (!self.extra_packet && self.packet_i >= 3) {
                if ! flags.contains(X_OVERFLOW) && ! flags.contains(Y_OVERFLOW) {
                    let mut dx = self.packets[1] as i32;
                    if flags.contains(X_SIGN) {
                        dx -= 0x100;
                    }

                    let mut dy = -(self.packets[2] as i32);
                    if flags.contains(Y_SIGN) {
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

                    let x = cmp::max(0, cmp::min(self.width as i32, self.mouse_x + dx));
                    let y = cmp::max(0, cmp::min(self.height as i32, self.mouse_y + dy));
                    if x != self.mouse_x || y != self.mouse_y {
                        self.mouse_x = x;
                        self.mouse_y = y;
                        self.input.write(&MouseEvent {
                            x: x,
                            y: y,
                        }.to_event()).expect("ps2d: failed to write mouse event");
                    }

                    if dz != 0 {
                        self.input.write(&ScrollEvent {
                            x: 0,
                            y: dz,
                        }.to_event()).expect("ps2d: failed to write scroll event");
                    }

                    let left = flags.contains(LEFT_BUTTON);
                    let middle = flags.contains(MIDDLE_BUTTON);
                    let right = flags.contains(RIGHT_BUTTON);
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
                    println!("ps2d: overflow {:X} {:X} {:X} {:X}", self.packets[0], self.packets[1], self.packets[2], self.packets[3]);
                }

                self.packets = [0; 4];
                self.packet_i = 0;
            }
        }
    }
}

// TODO:
// - check if file is open or not? Depends whether all uids can write or not
impl SchemeMut for Ps2d {
    #[allow(unused_variables)]
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if path == b"keymap" {
            self.open = true;
            Ok(0)
        } else {
            Err(Error::new(ENOENT))
        }
    }

    #[allow(unused_variables)]
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let until = cmp::min(self.pos*4 + buf.len(), MAX_KEYCODE*N_MOD_COMBOS*4);
        let len = until - self.pos;
        if len % 4 != 0 {
            return Err(Error::new(EINVAL));
        }
        for i in 0 .. len/4 {
            let u = u32::from(self.keymap.get_char_at(self.pos));
            BigEndian::write_u32(&mut buf[i*4..], u);
            self.pos += 1;
        }
        Ok(len)
        // Err(Error::new(EBADF))
    }

    #[allow(unused_variables)]
    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        let until = cmp::min(self.pos*4 + buf.len(), MAX_KEYCODE*N_MOD_COMBOS*4);
        let len = until - self.pos*4;
        if len % 4 != 0 {
            return Err(Error::new(EINVAL));
        }
        for i in 0 .. len/4 {
            let u: u32 = BigEndian::read_u32(&buf[i*4..]);
            let c: char = <char as TryFrom<u32>>::try_from(u).map_err(|_| Error::new(EINVAL))?;
            self.keymap.set_char_at(self.pos, c);
            self.pos += 1;
        }

        Ok(len)
    }

    #[allow(unused_variables)]
    fn seek(&mut self, id: usize, pos: usize, whence: usize) -> Result<usize> {
        // NOTE: Maybe a bit strange with all the `4`s. To the outside, the position is in bytes,
        // but here internally, self.pos is in chars (4 bytes). We only allow positioning on a
        // number divisible by 4.
        let new_pos = match whence {
            SEEK_SET => pos,
            SEEK_CUR => self.pos*4 + pos,
            SEEK_END => N_MOD_COMBOS * MAX_KEYCODE*4 - 4 - pos,
            _ => return Err(Error::new(EINVAL)),
        };
        if new_pos % 4 != 0 {
            Err(Error::new(EINVAL))
        } else {
            self.pos = new_pos / 4;
            Ok(0)
        }
    }

    #[allow(unused_variables)]
    fn fmap(&mut self, id: usize, offset: usize, size: usize) -> Result<usize> {
        Err(Error::new(EBADF))
    }

    #[allow(unused_variables)]
    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let path = b"ps2:keymap";
        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }
        Ok(i)
    }

    #[allow(unused_variables)]
    fn close(&mut self, id: usize) -> Result<usize> {
        self.open = false;
        Ok(0)
    }
}

