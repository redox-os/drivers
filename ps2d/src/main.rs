#![feature(asm)]

#[macro_use]
extern crate bitflags;
extern crate orbclient;
extern crate syscall;

use std::{env, process};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

use syscall::iopl;

use crate::state::Ps2d;

mod controller;
mod keymap;
mod state;
mod vm;

fn daemon(input: File) {
    unsafe {
        iopl(3).expect("ps2d: failed to get I/O permission");
    }

    let keymap = match env::args().skip(1).next() {
        Some(k) => match k.to_lowercase().as_ref() {
            "dvorak" => (keymap::dvorak::get_char),
            "us" => (keymap::us::get_char),
            "gb" => (keymap::gb::get_char),
            "azerty" => (keymap::azerty::get_char),
            "bepo" => (keymap::bepo::get_char),
            "it" => (keymap::it::get_char),
            &_ => (keymap::us::get_char)
        },
        None => (keymap::us::get_char)
    };

    let mut event_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(syscall::O_NONBLOCK as i32)
        .open("event:")
        .expect("ps2d: failed to open event:");

    let mut key_irq = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(syscall::O_NONBLOCK as i32)
        .open("irq:1")
        .expect("ps2d: failed to open irq:1");

    let mut key_irq_data = [0; 8];
    key_irq.read(&mut key_irq_data).expect("ps2d: failed to read irq:1");

    event_file.write(&syscall::Event {
        id: key_irq.as_raw_fd() as usize,
        flags: syscall::EVENT_READ,
        data: 1
    }).expect("ps2d: failed to event irq:1");

    key_irq.write(&key_irq_data).expect("ps2d: failed to write irq:1");

    let mut mouse_irq = OpenOptions::new()
        .read(true)
        .write(true)
        .open("irq:12")
        .expect("ps2d: failed to open irq:12");

    let mut mouse_irq_data = [0; 8];
    mouse_irq.read(&mut mouse_irq_data).expect("ps2d: failed to read irq:12");

    event_file.write(&syscall::Event {
        id: mouse_irq.as_raw_fd() as usize,
        flags: syscall::EVENT_READ,
        data: 1
    }).expect("ps2d: failed to event irq:12");

    mouse_irq.write(&mouse_irq_data).expect("ps2d: failed to write irq:12");

    let mut ps2d = Ps2d::new(input, keymap);

    syscall::setrens(0, 0).expect("ps2d: failed to enter null namespace");

    loop {
        // There are some gotchas with ps/2 controllers that require this weird
        // way of doing things. You read key and mouse data from the same
        // place. There is a status register that may show you which the data
        // came from, but if it is even implemented it can have a race
        // condition causing keyboard data to be read as mouse data.
        //
        // So, if any IRQ is returned as an event, first we check if a keyboard
        // IRQ has happened. If so, we know the next byte is keyboard data. If
        // not, we can read mouse data.

        let mut event = syscall::Event::default();
        if event_file.read(&mut event).expect("ps2d: failed to read event file") == 0 {
            break;
        }

        let last_mouse_irq_data = mouse_irq_data;
        mouse_irq.read(&mut mouse_irq_data).expect("ps2d: failed to read irq:12");
        let mouse_irq_change =  mouse_irq_data != last_mouse_irq_data;

        let last_key_irq_data = key_irq_data;
        key_irq.read(&mut key_irq_data).expect("ps2d: failed to read irq:1");
        let key_irq_change = key_irq_data != last_key_irq_data;

        if key_irq_change {
            ps2d.irq(true);
            key_irq.write(&key_irq_data).expect("ps2d: failed to write irq:1");
        } else if mouse_irq_change {
            ps2d.irq(false);
        } else {
            println!("ps2d: no irq change found");
        }

        if mouse_irq_change {
            mouse_irq.write(&mouse_irq_data).expect("ps2d: failed to write irq:12");
        }
    }
}

fn main() {
    match OpenOptions::new().write(true).open("display:input") {
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
