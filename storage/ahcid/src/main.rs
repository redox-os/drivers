#![cfg_attr(target_arch = "aarch64", feature(stdsimd))] // Required for yield instruction

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::usize;

use common::io::Io;
use driver_block::{DiskScheme, ExecutorTrait, FuturesExecutor};
use event::{EventFlags, RawEventQueue};
use pcid_interface::PciFunctionHandle;

use log::{error, info};

pub mod ahci;

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ahcid: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ahci");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ahcid: no legacy interrupts supported");

    common::setup_logging(
        "disk",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    info!("AHCI {}", pci_config.func.display());

    let address = unsafe { pcid_handle.map_bar(5) }.ptr.as_ptr() as usize;
    {
        let (hba_mem, disks) = ahci::disks(address as usize, &name);

        let scheme_name = format!("disk.{}", name);
        let mut scheme = DiskScheme::new(
            Some(daemon),
            scheme_name,
            disks
                .into_iter()
                .enumerate()
                .map(|(i, disk)| (i as u32, disk))
                .collect(),
            &FuturesExecutor,
        );

        let mut irq_file = irq.irq_handle("ahcid");
        let irq_fd = irq_file.as_raw_fd() as usize;

        let event_queue = RawEventQueue::new().expect("ahcid: failed to create event queue");

        libredox::call::setrens(0, 0).expect("ahcid: failed to enter null namespace");

        event_queue
            .subscribe(scheme.event_handle().raw(), 1, EventFlags::READ)
            .expect("ahcid: failed to event scheme socket");
        event_queue
            .subscribe(irq_fd, 1, EventFlags::READ)
            .expect("ahcid: failed to event irq scheme");

        for event in event_queue {
            let event = event.unwrap();
            if event.fd == scheme.event_handle().raw() {
                FuturesExecutor.block_on(scheme.tick()).unwrap();
            } else if event.fd == irq_fd {
                let mut irq = [0; 8];
                if irq_file
                    .read(&mut irq)
                    .expect("ahcid: failed to read irq file")
                    >= irq.len()
                {
                    let is = hba_mem.is.read();
                    if is > 0 {
                        let pi = hba_mem.pi.read();
                        let pi_is = pi & is;
                        for i in 0..hba_mem.ports.len() {
                            if pi_is & 1 << i > 0 {
                                let port = &mut hba_mem.ports[i];
                                let is = port.is.read();
                                port.is.write(is);
                            }
                        }
                        hba_mem.is.write(is);

                        irq_file
                            .write(&irq)
                            .expect("ahcid: failed to write irq file");

                        FuturesExecutor.block_on(scheme.tick()).unwrap();
                    }
                }
            } else {
                error!("Unknown event {}", event.fd);
            }
        }
    }

    std::process::exit(0);
}
