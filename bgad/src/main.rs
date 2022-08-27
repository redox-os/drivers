#![deny(warnings)]

extern crate orbclient;
extern crate syscall;

use std::env;
use std::fs::File;
use std::io::{Read, Write};

use syscall::call::iopl;
use syscall::data::Packet;
use syscall::scheme::SchemeMut;

use crate::bga::Bga;
use crate::scheme::BgaScheme;

mod bga;
mod scheme;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("bgad: no name provided");
    name.push_str("_bga");

    let bar_str = args.next().expect("bgad: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("bgad: failed to parse address");

    print!("{}", format!(" + BGA {} on: {:X}\n", name, bar));

    redox_daemon::Daemon::new(move |daemon| {
        unsafe { iopl(3).unwrap() };

        let mut socket = File::create(":bga").expect("bgad: failed to create bga scheme");

        let mut bga = Bga::new();
        print!("{}", format!("   - BGA {}x{}\n", bga.width(), bga.height()));

        let mut scheme = BgaScheme {
            bga: bga,
            display: File::open("display:input").ok()
        };

        scheme.update_size();

        syscall::setrens(0, 0).expect("bgad: failed to enter null namespace");

        daemon.ready().expect("bgad: failed to notify parent");

        loop {
            let mut packet = Packet::default();
            if socket.read(&mut packet).expect("bgad: failed to read events from bga scheme") == 0 {
                break;
            }
            scheme.handle(&mut packet);
            socket.write(&packet).expect("bgad: failed to write responses to bga scheme");
        }
        std::process::exit(0);
    }).expect("bgad: failed to daemonize");
}
