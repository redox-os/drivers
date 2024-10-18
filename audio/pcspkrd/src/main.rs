mod pcspkr;
mod scheme;

use std::fs::File;
use std::io::{Read, Write};

use syscall::call::iopl;
use syscall::data::Packet;
use syscall::scheme::SchemeMut;

use redox_daemon::Daemon;

use self::pcspkr::Pcspkr;
use self::scheme::PcspkrScheme;

fn main() {
    Daemon::new(move |daemon| {
        unsafe { iopl(3).unwrap() };

        let mut socket = File::create(":pcspkr").expect("pcspkrd: failed to create pcspkr scheme");
        daemon.ready().expect("failed to notify parent");

        let pcspkr = Pcspkr::new();
        println!(" + pcspkr");

        let mut scheme = PcspkrScheme {
            pcspkr,
            handle: None,
            next_id: 0,
        };

        libredox::call::setrens(0, 0).expect("pcspkrd: failed to enter null namespace");

        loop {
            let mut packet = Packet::default();
            socket
                .read(&mut packet)
                .expect("pcspkrd: failed to read events from pcspkr scheme");
            scheme.handle(&mut packet);
            socket
                .write(&packet)
                .expect("pcspkrd: failed to write responses to pcspkr scheme");
        }
    })
    .expect("pcspkrd: failed to daemonize");
}
