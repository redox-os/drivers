extern crate syscall;

use std::{env, slice};
use syscall::io::{Mmio, Io};

#[repr(packed)]
pub struct XhciCap {
    len: Mmio<u8>,
    _rsvd: Mmio<u8>,
    hci_ver: Mmio<u16>,
    hcs_params1: Mmio<u32>,
    hcs_params2: Mmio<u32>,
    hcs_params3: Mmio<u32>,
    hcc_params1: Mmio<u32>,
    db_offset: Mmio<u32>,
    rts_offset: Mmio<u32>,
    hcc_params2: Mmio<u32>
}

#[repr(packed)]
pub struct XhciOp {
    usb_cmd: Mmio<u32>,
    usb_std: Mmio<u32>,
    page_size: Mmio<u32>,
    _rsvd: [Mmio<u32>; 2],
    dn_ctrl: Mmio<u32>,
    crcr: [Mmio<u32>; 2],
    _rsvd2: [Mmio<u32>; 4],
    dcbaap: [Mmio<u32>; 2],
    config: Mmio<u32>,
}

pub struct Xhci {
    cap: &'static mut XhciCap,
    op: &'static mut XhciOp,
    ports: &'static mut [Mmio<u32>]
}

impl Xhci {
    pub fn new(address: usize) -> Xhci {
        let cap = unsafe { &mut *(address as *mut XhciCap) };

        let op_base = address + cap.len.read() as usize;
        let op = unsafe { &mut *(op_base as *mut XhciOp) };

        let port_base = op_base + 0x400;
        let port_len = ((cap.hcs_params1.read() & 0xFF000000) >> 24) as usize;
        let ports = unsafe { slice::from_raw_parts_mut(port_base as *mut Mmio<u32>, port_len) };

        Xhci {
            cap: cap,
            op: op,
            ports: ports
        }
    }
}

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

    let bar_str = args.next().expect("xhcid: no address provided");
    let bar = usize::from_str_radix(&bar_str, 16).expect("xhcid: failed to parse address");

    print!("{}", format!(" + XHCI {} on: {:X}\n", name, bar));

    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let address = unsafe { syscall::physmap(bar, 4096, syscall::MAP_WRITE).expect("xhcid: failed to map address") };
        {
            let mut xhci = Xhci::new(address);
            for (i, port) in xhci.ports.iter().enumerate() {
                println!("XHCI Port {}: {:X}", i, port.read());
            }
        }
        unsafe { let _ = syscall::physunmap(address); }
    }
}
