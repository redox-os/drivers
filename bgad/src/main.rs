#![deny(warnings)]

extern crate syscall;

use std::env;
use syscall::iopl;

use bga::Bga;

mod bga;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("bgad: no name provided");
    name.push_str("_bga");

    let bar_str = args.next().expect("bgad: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("bgad: failed to parse address");

    print!("{}", format!(" + BGA {} on: {:X}\n", name, bar));

    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        unsafe { iopl(3).unwrap() };

        let mut bga = Bga::new();
        print!("{}", format!("   - BGA {}x{}\n", bga.width(), bga.height()));
    }
}
