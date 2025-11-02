use common::io::Io as _;
use driver_block::{Disk, DiskScheme, ExecutorTrait, FuturesExecutor};
use event::{EventFlags, RawEventQueue};
use libredox::flag;
use log::{error, info};
use pcid_interface::PciFunctionHandle;
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::io::{FromRawFd, RawFd},
    sync::{Arc, Mutex},
    thread::{self, sleep},
    time::Duration,
};

use crate::ide::{AtaCommand, AtaDisk, Channel};

pub mod ide;

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ided: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let pcid_handle = PciFunctionHandle::connect_default();

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ide");

    common::setup_logging(
        "disk",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    info!("IDE PCI CONFIG: {:?}", pci_config);

    // Get controller DMA capable
    let dma = pci_config.func.full_device_id.interface & 0x80 != 0;

    let busmaster_base = pci_config.func.bars[4].expect_port();
    let (primary, primary_irq) = if pci_config.func.full_device_id.interface & 1 != 0 {
        panic!("TODO: IDE primary channel is PCI native");
    } else {
        (Channel::primary_compat(busmaster_base).unwrap(), 14)
    };
    let (secondary, secondary_irq) = if pci_config.func.full_device_id.interface & 1 != 0 {
        panic!("TODO: IDE secondary channel is PCI native");
    } else {
        (Channel::secondary_compat(busmaster_base + 8).unwrap(), 15)
    };

    common::acquire_port_io_rights().expect("ided: failed to get I/O privilege");

    //TODO: move this to ide.rs?
    let chans = vec![
        Arc::new(Mutex::new(primary)),
        Arc::new(Mutex::new(secondary)),
    ];
    enum AnyDisk {
        Ata(AtaDisk),
    }
    impl Disk for AnyDisk {
        fn block_size(&self) -> u32 {
            let AnyDisk::Ata(a) = self;
            a.block_size()
        }
        fn size(&self) -> u64 {
            let AnyDisk::Ata(a) = self;
            a.size()
        }
        async fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
            let AnyDisk::Ata(a) = self;
            a.write(block, buffer).await
        }
        async fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
            let AnyDisk::Ata(a) = self;
            a.read(block, buffer).await
        }
    }
    let mut disks: Vec<AnyDisk> = Vec::new();
    for (chan_i, chan_lock) in chans.iter().enumerate() {
        let mut chan = chan_lock.lock().unwrap();

        println!("  - channel {}", chan_i);

        // Disable IRQs
        chan.control.write(2);

        for dev in 0..=1 {
            println!("    - device {}", dev);

            // Select device
            chan.device_select.write(0xA0 | (dev << 4));
            sleep(Duration::from_millis(1));

            // ATA identify command
            chan.command.write(AtaCommand::Identify as u8);
            sleep(Duration::from_millis(1));

            // Check if device exists
            if chan.status.read() == 0 {
                println!("      not found");
                continue;
            }

            // Poll for status
            let error = loop {
                let status = chan.status.read();
                if status & 1 != 0 {
                    // Error
                    break true;
                }
                if status & 0x80 == 0 && status & 0x08 != 0 {
                    // Not busy and data ready
                    break false;
                }
                thread::yield_now();
            };

            //TODO: probe ATAPI
            if error {
                println!("      error");
                continue;
            }

            // Read and print identity
            {
                let mut dest = [0u16; 256];
                for chunk in dest.chunks_mut(2) {
                    let data = chan.data32.read();
                    chunk[0] = data as u16;
                    chunk[1] = (data >> 16) as u16;
                }

                let mut serial = String::new();
                for word in 10..20 {
                    let d = dest[word];
                    let a = ((d >> 8) as u8) as char;
                    if a != '\0' {
                        serial.push(a);
                    }
                    let b = (d as u8) as char;
                    if b != '\0' {
                        serial.push(b);
                    }
                }

                let mut firmware = String::new();
                for word in 23..27 {
                    let d = dest[word];
                    let a = ((d >> 8) as u8) as char;
                    if a != '\0' {
                        firmware.push(a);
                    }
                    let b = (d as u8) as char;
                    if b != '\0' {
                        firmware.push(b);
                    }
                }

                let mut model = String::new();
                for word in 27..47 {
                    let d = dest[word];
                    let a = ((d >> 8) as u8) as char;
                    if a != '\0' {
                        model.push(a);
                    }
                    let b = (d as u8) as char;
                    if b != '\0' {
                        model.push(b);
                    }
                }

                let mut sectors = (dest[100] as u64)
                    | ((dest[101] as u64) << 16)
                    | ((dest[102] as u64) << 32)
                    | ((dest[103] as u64) << 48);

                let lba_bits = if sectors == 0 {
                    sectors = (dest[60] as u64) | ((dest[61] as u64) << 16);
                    28
                } else {
                    48
                };

                println!("      Serial: {}", serial.trim());
                println!("      Firmware: {}", firmware.trim());
                println!("      Model: {}", model.trim());
                println!("      Size: {} MB", sectors / 2048);
                println!("      DMA: {}", dma);
                println!("      {}-bit LBA", lba_bits);

                disks.push(AnyDisk::Ata(AtaDisk {
                    chan: chan_lock.clone(),
                    chan_i,
                    dev,
                    size: sectors * 512,
                    dma,
                    lba_48: lba_bits == 48,
                }));
            }
        }
    }

    let scheme_name = format!("disk.{}", name);
    let mut scheme = DiskScheme::new(
        Some(daemon),
        scheme_name,
        disks
            .into_iter()
            .enumerate()
            .map(|(i, disk)| (i as u32, disk))
            .collect(),
        // TODO: Should ided just use TrivialExecutor or would it be valuable to actually use a
        // real executor?
        &FuturesExecutor,
    );

    let primary_irq_fd = libredox::call::open(
        &format!("/scheme/irq/{}", primary_irq),
        flag::O_RDWR | flag::O_NONBLOCK,
        0,
    )
    .expect("ided: failed to open irq file");
    let mut primary_irq_file = unsafe { File::from_raw_fd(primary_irq_fd as RawFd) };

    let secondary_irq_fd = libredox::call::open(
        &format!("/scheme/irq/{}", secondary_irq),
        flag::O_RDWR | flag::O_NONBLOCK,
        0,
    )
    .expect("ided: failed to open irq file");
    let mut secondary_irq_file = unsafe { File::from_raw_fd(secondary_irq_fd as RawFd) };

    let event_queue = RawEventQueue::new().expect("ided: failed to open event file");

    libredox::call::setrens(0, 0).expect("ided: failed to enter null namespace");

    event_queue
        .subscribe(scheme.event_handle().raw(), 0, EventFlags::READ)
        .expect("ided: failed to event disk scheme");

    event_queue
        .subscribe(primary_irq_fd, 0, EventFlags::READ)
        .expect("ided: failed to event irq scheme");

    event_queue
        .subscribe(secondary_irq_fd, 0, EventFlags::READ)
        .expect("ided: failed to event irq scheme");

    for event in event_queue {
        let event = event.unwrap();
        if event.fd == scheme.event_handle().raw() {
            FuturesExecutor.block_on(scheme.tick()).unwrap();
        } else if event.fd == primary_irq_fd {
            let mut irq = [0; 8];
            if primary_irq_file
                .read(&mut irq)
                .expect("ided: failed to read irq file")
                >= irq.len()
            {
                let _chan = chans[0].lock().unwrap();
                //TODO: check chan for irq

                primary_irq_file
                    .write(&irq)
                    .expect("ided: failed to write irq file");

                FuturesExecutor.block_on(scheme.tick()).unwrap();
            }
        } else if event.fd == secondary_irq_fd {
            let mut irq = [0; 8];
            if secondary_irq_file
                .read(&mut irq)
                .expect("ided: failed to read irq file")
                >= irq.len()
            {
                let _chan = chans[1].lock().unwrap();
                //TODO: check chan for irq

                secondary_irq_file
                    .write(&irq)
                    .expect("ided: failed to write irq file");

                FuturesExecutor.block_on(scheme.tick()).unwrap();
            }
        } else {
            error!("Unknown event {}", event.fd);
        }
    }

    std::process::exit(0);
}
