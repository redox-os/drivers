use std::{fs::File, io::{Read, Write}, process};

use driver_block::Disk;
use fdt::{Fdt, node::FdtNode};
use syscall::{Packet, SchemeBlockMut};

use crate::scheme::DiskScheme;

mod sd;
mod scheme;

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
    println!("DeviceMemory start = 0x{:08x}, size = 0x{:08x}", reg.starting_address as usize, reg_size);
    let addr = unsafe {
        common::physmap(reg.starting_address as usize, reg_size, common::Prot::RW, common::MemoryType::DeviceMemory)
            .expect("bcm2835-sdhcid: failed to map address") as usize
    };
    println!("ioremap 0x{:08x} to 0x{:08x} 2222", reg.starting_address as usize, addr);
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


    let scheme_name = ":disk.mmc";
    let mut socket = File::create(scheme_name).expect("mmc: failed to create disk scheme");
    libredox::call::setrens(0, 0).expect("mmc: failed to enter null namespace");

    daemon.ready().expect("mmc: failed to notify parent");

    let mut todo = Vec::new();
    let mut disks = Vec::new();

    disks.push(Box::new(sdhci) as Box<dyn Disk>);
    let mut scheme = DiskScheme::new(scheme_name.to_string(), disks);
    loop {
        let mut packet = Packet::default();
        if socket.read(&mut packet).expect("mmc: failed to read event") == 0 {
            println!("zero, break");
            break;
        }
        if let Some(a) = scheme.handle(&packet) {
            packet.a = a;
            socket.write(&packet).expect("mmcd: failed to write disk scheme");
        } else {
            todo.push(packet);
        }
        let mut i = 0;
        while i < todo.len() {
            if let Some(a) = scheme.handle(&todo[i]) {
                let mut packet = todo.remove(i);
                packet.a = a;
                socket.write(&packet).expect("mmcd: failed to write disk scheme");
            } else {
                i += 1;
            }
        }
    }
    process::exit(0);
}
