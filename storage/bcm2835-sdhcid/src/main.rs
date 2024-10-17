use std::{
    fs::File,
    io::{Read, Write},
    process,
};

use driver_block::Disk;
use event::{EventFlags, RawEventQueue};
use fdt::{node::FdtNode, Fdt};
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket, V2};
use syscall::{
    data::{Event, Packet},
    error::{Error, ENODEV},
    flag::EVENT_READ,
    io::Io,
    scheme::SchemeBlockMut,
    EAGAIN, EINTR, EWOULDBLOCK,
};

use crate::scheme::DiskScheme;

mod scheme;
mod sd;

#[cfg(target_os = "redox")]
fn get_dtb() -> Vec<u8> {
    std::fs::read("kernel.dtb:").unwrap()
}

#[cfg(target_os = "linux")]
fn get_dtb() -> Vec<u8> {
    use std::env;
    if let Some(arg1) = env::args().nth(1) {
        std::fs::read(arg1).unwrap()
    } else {
        Vec::new()
    }
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("mmc:failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let dtb_data = get_dtb();
    println!("read from OS, len = {}", dtb_data.len());
    if dtb_data.len() == 0 {
        process::exit(0);
    }

    let fdt = Fdt::new(&dtb_data).unwrap();
    println!("DTB model = {}", fdt.root().model());
    let with = ["brcm,bcm2835-sdhcid"];
    let compat_node = fdt.find_compatible(&with).unwrap();
    let reg = compat_node.reg().unwrap().next().unwrap();
    let reg_size = reg.size.unwrap();
    println!(
        "DeviceMemory start = 0x{:08x}, size = 0x{:08x}",
        reg.starting_address as usize, reg_size
    );
    let addr = unsafe {
        common::physmap(
            reg.starting_address as usize,
            reg_size,
            common::Prot::RW,
            common::MemoryType::DeviceMemory,
        )
        .expect("bcm2835-sdhcid: failed to map address") as usize
    };
    println!(
        "ioremap 0x{:08x} to 0x{:08x} 2222",
        reg.starting_address as usize, addr
    );
    let mut sdhci = sd::SdHostCtrl::new(addr);
    unsafe {
        sdhci.init();
        /*
        let mut buf1 = [0u32; 512];
        sdhci.sd_readblock(1, &mut buf1, 1);
        println!("readblock {:?}", buf1);
        buf1[0] = 0xdead_0000;
        buf1[1] = 0xdead_0000;
        buf1[2] = 0x0000_dead;
        buf1[3] = 0x0000_dead;
        sdhci.sd_writeblock(1, &buf1, 1);
        sdhci.sd_readblock(1, &mut buf1, 1);
        println!("readblock {:?}", buf1);
        */
        /*
        let mut buf1 = [0u8; 512];
        sdhci.read(1, &mut buf1);
        println!("readblock {:?}", buf1);
        buf1[0] = 0xde;
        buf1[1] = 0xad;
        buf1[2] = 0xde;
        buf1[3] = 0xad;
        sdhci.write(1, &buf1);
        sdhci.read(1, &mut buf1);
        println!("readblock {:?}", buf1);
        */
    }

    let scheme_name = "disk.mmc";
    let socket_fd = Socket::<V2>::create(&scheme_name).expect("mmcd: failed to create disk scheme");

    let mut event_queue = RawEventQueue::new().expect("mmcd: failed to open event file");
    event_queue
        .subscribe(socket_fd.inner().raw(), 0, EventFlags::READ)
        .expect("mmcd: failed to event disk scheme");

    libredox::call::setrens(0, 0).expect("mmcd: failed to enter null namespace");
    daemon.ready().expect("mmcd: failed to notify parent");

    let mut todo = Vec::new();
    let mut disks = Vec::new();

    disks.push(Box::new(sdhci) as Box<dyn Disk>);
    let mut scheme = DiskScheme::new(scheme_name.to_string(), disks);

    'outer: loop {
        let Some(event) = event_queue
            .next()
            .transpose()
            .expect("mmcd: failed to read event file")
        else {
            break;
        };
        if event.fd == socket_fd.inner().raw() {
            loop {
                let req = match socket_fd.next_request(SignalBehavior::Interrupt) {
                    Ok(None) => break 'outer,
                    Ok(Some(r)) => {
                        if let RequestKind::Call(c) = r.kind() {
                            c
                        } else {
                            continue;
                        }
                    }
                    Err(err) => {
                        if matches!(err.errno, EAGAIN | EWOULDBLOCK | EINTR) {
                            break;
                        } else {
                            panic!("mmcd: failed to read disk scheme: {}", err);
                        }
                    }
                };
                if let Some(resp) = req.handle_scheme_block_mut(&mut scheme) {
                    socket_fd
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("mmcd: failed to write disk scheme");
                } else {
                    todo.push(req);
                }
            }
        } else {
            println!("Unknown event {}", event.fd);
        }

        // Handle todos to start new packets if possible
        let mut i = 0;
        while i < todo.len() {
            if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                socket_fd
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("mmcd: failed to write disk scheme");
            } else {
                i += 1;
            }
        }

        for req in todo.drain(..) {
            socket_fd
                .write_response(
                    Response::new(&req, Err(Error::new(ENODEV))),
                    SignalBehavior::Restart,
                )
                .expect("mmcd: failed to write disk scheme");
        }
    }
    process::exit(0);
}
