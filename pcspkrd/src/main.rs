mod pcspkr;
mod scheme;

use std::fs::File;
use std::io::{Read, Write};

use syscall::call::iopl;
use syscall::data::Packet;
use syscall::flag::CloneFlags;
use syscall::scheme::SchemeMut;

use self::pcspkr::Pcspkr;
use self::scheme::PcspkrScheme;

fn main() {
    // Daemonize
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } == 0 {
        unsafe { iopl(3).unwrap() };

        let mut socket = File::create(":pcspkr").expect("pcspkrd: failed to create pcspkr scheme");

        let pcspkr = Pcspkr::new();
        println!(" + pcspkr");

        let mut scheme = PcspkrScheme {
            pcspkr: pcspkr,
            handle: None,
            next_id: 0,
        };

        syscall::setrens(0, 0).expect("pcspkrd: failed to enter null namespace");

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
    }
}
