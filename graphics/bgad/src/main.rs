extern crate orbclient;
extern crate syscall;

use std::fs::File;
use std::io::{Read, Write};

use pcid_interface::PciFunctionHandle;
use syscall::call::iopl;
use syscall::data::Packet;
use syscall::scheme::SchemeMut;

use crate::bga::Bga;
use crate::scheme::BgaScheme;

mod bga;
mod scheme;

fn main() {
    let pcid_handle =
        PciFunctionHandle::connect_default().expect("bgad: failed to setup channel to pcid");
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_bga");

    println!(" + BGA {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        unsafe { iopl(3).unwrap() };

        let mut socket = File::create(":bga").expect("bgad: failed to create bga scheme");

        let mut bga = Bga::new();
        println!("   - BGA {}x{}", bga.width(), bga.height());

        let mut scheme = BgaScheme {
            bga,
            display: File::open("/scheme/input/producer").ok(),
        };

        scheme.update_size();

        libredox::call::setrens(0, 0).expect("bgad: failed to enter null namespace");

        daemon.ready().expect("bgad: failed to notify parent");

        loop {
            let mut packet = Packet::default();
            if socket
                .read(&mut packet)
                .expect("bgad: failed to read events from bga scheme")
                == 0
            {
                break;
            }
            scheme.handle(&mut packet);
            socket
                .write(&packet)
                .expect("bgad: failed to write responses to bga scheme");
        }
        std::process::exit(0);
    })
    .expect("bgad: failed to daemonize");
}
