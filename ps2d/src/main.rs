#![deny(warnings)]
#![feature(asm)]

#[macro_use]
extern crate bitflags;
extern crate event;
extern crate orbclient;
extern crate syscall;

use std::{cmp, env, process};
use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Write, Result};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use event::EventQueue;
use orbclient::{KeyEvent, MouseEvent, ButtonEvent, ScrollEvent};
use syscall::iopl;

use controller::Ps2;

mod controller;
mod keymap;

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

struct Ps2d<F: Fn(u8,bool,bool) -> char>  {
    ps2: Ps2,
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
    //Keymap function
    get_char: F,
    // state to switch to PS/2 Scan Code Set 2
    scancode_set_2: bool,
}

impl<F: Fn(u8,bool,bool) -> char> Ps2d<F> {
    fn new(input: File, keymap: F) -> Self {
        let mut ps2 = Ps2::new();
        let extra_packet = ps2.init();

        let mut width = 0;
        let mut height = 0;
        {
            let mut buf: [u8; 4096] = [0; 4096];
            if let Ok(count) = syscall::fpath(input.as_raw_fd() as usize, &mut buf) {
                let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
                let res = path.split(":").nth(1).unwrap_or("");
                width = res.split("/").nth(1).unwrap_or("").parse::<u32>().unwrap_or(0);
                height = res.split("/").nth(2).unwrap_or("").parse::<u32>().unwrap_or(0);
            }
        }

        Ps2d {
            ps2: ps2,
            input: input,
            width: width,
            height: height,
            lshift: false,
            rshift: false,
            alt_gr: false,
            mouse_x: 0,
            mouse_y: 0,
            mouse_left: false,
            mouse_middle: false,
            mouse_right: false,
            scancode_set_2: false,
            packets: [0; 4],
            packet_i: 0,
            extra_packet: extra_packet,
            get_char: keymap
        }
        
    }

    fn irq(&mut self) {
        while let Some((keyboard, data)) = self.ps2.next() {
            if keyboard {
                self.handle_keyboard(data);
            }
            else {
                self.handle_mouse(data);
            }
        }
    }

    fn handle_keyboard(&mut self, data: u8) {
        // The scancode is at least in two part
        // Switch the state machine so that with the next call we get the next piece of the scancode
        if data == 0xE0 {
            self.scancode_set_2 = !self.scancode_set_2;
        } else {
             let (scancode, pressed) = if data >= 0x80 {
               (data - 0x80, false)
            } else   {
               (data, true)
            };

            // Previous call switched the state machine
            if self.scancode_set_2 {
                // set the state machine to the original state
                self.scancode_set_2 = false;

                // 0xE0, 0x38 is Alt_gr scancode
                if scancode == 0x38 {
                    self.alt_gr = pressed;
                    // We ignore this key_event further
                    // This is to avoid a problem with orbital/src/scheme.rs
                    return ;
                }
            }

            if scancode == 0x2A {
                self.lshift = pressed;
            } else if scancode == 0x36 {
                self.rshift = pressed;
            }

            self.input.write(&KeyEvent {
                character: (self.get_char)(scancode, self.lshift || self.rshift, self.alt_gr),
                scancode: scancode,
                pressed: pressed
            }.to_event()).expect("ps2d: failed to write key event");
        }
    }
    
    fn handle_mouse(&mut self, data: u8) {
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

                if dz != 0 {
                    self.input.write(&ScrollEvent {
                        x: 0,
                        y: dz,
                    }.to_event()).expect("ps2d: failed to write scroll event");
                }
            } else {
                println!("ps2d: overflow {:X} {:X} {:X} {:X}", self.packets[0], self.packets[1], self.packets[2], self.packets[3]);
            }

            self.packets = [0; 4];
            self.packet_i = 0;
        }
    }
}

fn daemon(input: File) {
    unsafe {
        iopl(3).expect("ps2d: failed to get I/O permission");
    }

    let keymap = match env::args().skip(1).next() {
        Some(k) => match k.to_lowercase().as_ref() {
            "dvorak" => (keymap::dvorak::get_char),
            "english" => (keymap::english::get_char),
            "azerty" => (keymap::azerty::get_char),
            "bepo" => (keymap::bepo::get_char),
            &_ => (keymap::english::get_char)
        },
        None => (keymap::english::get_char)
    };
    let ps2d = Arc::new(RefCell::new(Ps2d::new(input, keymap)));

    let mut event_queue = EventQueue::<()>::new().expect("ps2d: failed to create event queue");

    let mut key_irq = File::open("irq:1").expect("ps2d: failed to open irq:1");
    let key_ps2d = ps2d.clone();
    event_queue.add(key_irq.as_raw_fd(), move |_count: usize| -> Result<Option<()>> {
        let mut irq = [0; 8];
        if key_irq.read(&mut irq)? >= irq.len() {
            key_ps2d.borrow_mut().irq();
            key_irq.write(&irq)?;
        }
        Ok(None)
    }).expect("ps2d: failed to poll irq:1");

    let mut mouse_irq = File::open("irq:12").expect("ps2d: failed to open irq:12");
    let mouse_ps2d = ps2d;
    event_queue.add(mouse_irq.as_raw_fd(), move |_count: usize| -> Result<Option<()>> {
        let mut irq = [0; 8];
        if mouse_irq.read(&mut irq)? >= irq.len() {
            mouse_ps2d.borrow_mut().irq();
            mouse_irq.write(&irq)?;
        }
        Ok(None)
    }).expect("ps2d: failed to poll irq:12");

    event_queue.trigger_all(0).expect("ps2d: failed to trigger events");

    event_queue.run().expect("ps2d: failed to handle events");
}

fn main() {
    match File::open("display:input") {
        Ok(input) => {
            // Daemonize
            if unsafe { syscall::clone(0).unwrap() } == 0 {
                daemon(input);
            }
        },
        Err(err) => {
            println!("ps2d: failed to open display: {}", err);
            process::exit(1);
        }
    }
}
