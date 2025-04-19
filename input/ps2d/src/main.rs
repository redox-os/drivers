#[macro_use]
extern crate bitflags;
extern crate orbclient;
extern crate syscall;

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::{env, process};

use common::acquire_port_io_rights;
use event::{user_data, EventQueue};
use inputd::ProducerHandle;
use log::info;

use crate::state::Ps2d;

mod controller;
mod keymap;
mod state;
mod vm;

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    common::setup_logging(
        "misc",
        "ps2",
        "ps2",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    acquire_port_io_rights().expect("ps2d: failed to get I/O permission");

    let (keymap, keymap_name): (fn(u8, bool) -> char, &str) = match env::args().skip(1).next() {
        Some(k) => match k.to_lowercase().as_ref() {
            "dvorak" => (keymap::dvorak::get_char, "dvorak"),
            "us" => (keymap::us::get_char, "us"),
            "gb" => (keymap::gb::get_char, "gb"),
            "azerty" => (keymap::azerty::get_char, "azerty"),
            "bepo" => (keymap::bepo::get_char, "bepo"),
            "it" => (keymap::it::get_char, "it"),
            &_ => (keymap::us::get_char, "us"),
        },
        None => (keymap::us::get_char, "us"),
    };

    info!("ps2d: using keymap '{}'", keymap_name);

    let input = ProducerHandle::new().expect("ps2d: failed to open input producer");

    user_data! {
        enum Source {
            Keyboard,
            Mouse,
        }
    }

    let event_queue: EventQueue<Source> =
        EventQueue::new().expect("ps2d: failed to create event queue");

    let mut key_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(syscall::O_NONBLOCK as i32)
        .open("/scheme/serio/0")
        .expect("ps2d: failed to open /scheme/serio/0");

    event_queue
        .subscribe(
            key_file.as_raw_fd() as usize,
            Source::Keyboard,
            event::EventFlags::READ,
        )
        .unwrap();

    let mut mouse_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(syscall::O_NONBLOCK as i32)
        .open("/scheme/serio/1")
        .expect("ps2d: failed to open /scheme/serio/1");

    event_queue
        .subscribe(
            mouse_file.as_raw_fd() as usize,
            Source::Mouse,
            event::EventFlags::READ,
        )
        .unwrap();

    libredox::call::setrens(0, 0).expect("ps2d: failed to enter null namespace");

    daemon
        .ready()
        .expect("ps2d: failed to mark daemon as ready");

    let mut ps2d = Ps2d::new(input, keymap);

    let mut data = [0; 256];
    for event in event_queue.map(|e| e.expect("ps2d: failed to get next event").user_data) {
        // There are some gotchas with ps/2 controllers that require this weird
        // way of doing things. You read key and mouse data from the same
        // place. There is a status register that may show you which the data
        // came from, but if it is even implemented it can have a race
        // condition causing keyboard data to be read as mouse data.
        //
        // Due to this, we have a kernel driver doing a small amount of work
        // to grab bytes and sort them based on the source

        let (file, keyboard) = match event {
            Source::Keyboard => (&mut key_file, true),
            Source::Mouse => (&mut mouse_file, false),
        };

        loop {
            let count = match file.read(&mut data) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => break,
            };
            for i in 0..count {
                ps2d.handle(keyboard, data[i]);
            }
        }
    }

    process::exit(0);
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ps2d: failed to create daemon");
}
