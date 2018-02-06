#![deny(warnings)]
#![feature(asm)]

#[macro_use]
extern crate bitflags;
extern crate event;
extern crate orbclient;
extern crate syscall;

use std::{env, process};
use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Write, Result};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use event::EventQueue;
use syscall::iopl;

use state::Ps2d;

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
            &_ => (keymap::us::get_char)
        },
        None => (keymap::us::get_char)
    };

    let mut key_irq = File::open("irq:1").expect("ps2d: failed to open irq:1");

    let mut mouse_irq = File::open("irq:12").expect("ps2d: failed to open irq:12");

    let ps2d = Arc::new(RefCell::new(Ps2d::new(input, keymap)));

    let mut event_queue = EventQueue::<()>::new().expect("ps2d: failed to create event queue");

    syscall::setrens(0, 0).expect("ps2d: failed to enter null namespace");

    let key_ps2d = ps2d.clone();
    event_queue.add(key_irq.as_raw_fd(), move |_count: usize| -> Result<Option<()>> {
        let mut irq = [0; 8];
        if key_irq.read(&mut irq)? >= irq.len() {
            key_ps2d.borrow_mut().irq();
            key_irq.write(&irq)?;
        }
        Ok(None)
    }).expect("ps2d: failed to poll irq:1");

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
