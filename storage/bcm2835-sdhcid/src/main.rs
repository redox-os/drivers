use std::process;

use driver_block::{DiskScheme, ExecutorTrait, TrivialExecutor};
use event::{EventFlags, RawEventQueue};
use fdt::Fdt;

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
    let with = ["brcm,bcm2835-sdhci"];
    let compat_node = fdt.find_compatible(&with).unwrap();
    let reg = compat_node.reg().unwrap().next().unwrap();
    let reg_size = reg.size.unwrap();
    let mut reg_addr = reg.starting_address as usize;
    println!(
        "DeviceMemory start = 0x{:08x}, size = 0x{:08x}",
        reg_addr, reg_size
    );
    if let Some(mut ranges) = fdt.find_node("/soc").and_then(|f| f.ranges()) {
        let range = ranges
            .find(|f| f.child_bus_address <= reg_addr && reg_addr - f.child_bus_address < f.size)
            .expect("Couldn't find device range in /soc/@ranges");
        reg_addr = range.parent_bus_address + (reg_addr - range.child_bus_address);
        println!(
            "DeviceMemory remapped onto CPU address space: start = 0x{:08x}, size = 0x{:08x}",
            reg_addr, reg_size
        );
    }

    let addr = unsafe {
        common::physmap(
            reg_addr,
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

    let mut disks = Vec::new();
    disks.push(sdhci);
    let mut scheme = DiskScheme::new(
        Some(daemon),
        "disk.mmc".to_string(),
        disks
            .into_iter()
            .enumerate()
            .map(|(i, disk)| (i as u32, disk))
            .collect(),
        &TrivialExecutor, // TODO: real executor
    );

    let event_queue = RawEventQueue::new().expect("mmcd: failed to open event file");
    event_queue
        .subscribe(scheme.event_handle().raw(), 0, EventFlags::READ)
        .expect("mmcd: failed to event disk scheme");

    libredox::call::setrens(0, 0).expect("mmcd: failed to enter null namespace");

    for event in event_queue {
        let event = event.unwrap();
        if event.fd == scheme.event_handle().raw() {
            TrivialExecutor.block_on(scheme.tick()).unwrap();
        } else {
            println!("Unknown event {}", event.fd);
        }
    }
    process::exit(0);
}
