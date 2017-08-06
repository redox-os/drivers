#[macro_use]
extern crate bitflags;
extern crate plain;
extern crate syscall;

use std::env;

use xhci::Xhci;

mod usb;
mod xhci;

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

    let bar_str = args.next().expect("xhcid: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("xhcid: failed to parse address");

    print!("{}", format!(" + XHCI {} on: {:X}\n", name, bar));

    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let address = unsafe { syscall::physmap(bar, 65536, syscall::MAP_WRITE).expect("xhcid: failed to map address") };

        match Xhci::new(address) {
            Ok(mut xhci) => {
                if let Err(err) = xhci.probe() {
                    println!("xhcid: probe error: {}", err);
                }
            },
            Err(err) => {
                println!("xhcid: open error: {}", err);
            }
        }

        unsafe { let _ = syscall::physunmap(address); }
    }
}
