use common::io::Io as _;
use driver_block::Disk;
use event::{EventFlags, RawEventQueue};
use libredox::flag;
use log::{error, info};
use pcid_interface::{PciBar, PciFunctionHandle};
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket, V2};
use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::unix::io::{FromRawFd, RawFd},
    sync::{Arc, Mutex},
    thread::{self, sleep},
    time::Duration,
};
use syscall::{
    data::{Event, Packet},
    error::{Error, ENODEV},
    flag::EVENT_READ,
    scheme::SchemeBlockMut,
    EAGAIN, EINTR, EWOULDBLOCK,
};

use crate::{
    ide::{AtaCommand, AtaDisk, Channel},
    scheme::DiskScheme,
};

pub mod ide;
pub mod scheme;

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ided: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let pcid_handle =
        PciFunctionHandle::connect_default().expect("ided: failed to setup channel to pcid");

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ide");

    common::setup_logging(
        "disk",
        "pcie",
        &name,
        log::LevelFilter::Info,
        log::LevelFilter::Info,
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
    let mut disks: Vec<Box<dyn Disk>> = Vec::new();
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

                disks.push(Box::new(AtaDisk {
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
    let socket_fd =
        Socket::<V2>::nonblock(&scheme_name).expect("ided: failed to create disk scheme");

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

    let mut event_queue = RawEventQueue::new().expect("ided: failed to open event file");

    libredox::call::setrens(0, 0).expect("ided: failed to enter null namespace");

    daemon.ready().expect("ided: failed to notify parent");

    event_queue
        .subscribe(socket_fd.inner().raw(), 0, EventFlags::READ)
        .expect("ided: failed to event disk scheme");

    event_queue
        .subscribe(primary_irq_fd, 0, EventFlags::READ)
        .expect("ided: failed to event irq scheme");

    event_queue
        .subscribe(secondary_irq_fd, 0, EventFlags::READ)
        .expect("ided: failed to event irq scheme");

    let mut scheme = DiskScheme::new(scheme_name, chans, disks);

    let mut todo = Vec::new();
    'outer: loop {
        let Some(event) = event_queue
            .next()
            .transpose()
            .expect("ided: failed to read event file")
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
                            panic!("ided: failed to read disk scheme: {}", err);
                        }
                    }
                };
                if let Some(resp) = req.handle_scheme_block_mut(&mut scheme) {
                    socket_fd
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("ided: failed to write disk scheme");
                } else {
                    todo.push(req);
                }
            }
        } else if event.fd == primary_irq_fd {
            let mut irq = [0; 8];
            if primary_irq_file
                .read(&mut irq)
                .expect("ided: failed to read irq file")
                >= irq.len()
            {
                if scheme.irq(0) {
                    primary_irq_file
                        .write(&irq)
                        .expect("ided: failed to write irq file");

                    // Handle todos in order to finish previous packets if possible
                    let mut i = 0;
                    while i < todo.len() {
                        if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                            socket_fd
                                .write_response(resp, SignalBehavior::Restart)
                                .expect("ided: failed to write disk scheme");
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        } else if event.fd == secondary_irq_fd {
            let mut irq = [0; 8];
            if secondary_irq_file
                .read(&mut irq)
                .expect("ided: failed to read irq file")
                >= irq.len()
            {
                if scheme.irq(1) {
                    secondary_irq_file
                        .write(&irq)
                        .expect("ided: failed to write irq file");

                    // Handle todos in order to finish previous packets if possible
                    let mut i = 0;
                    while i < todo.len() {
                        if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                            socket_fd
                                .write_response(resp, SignalBehavior::Restart)
                                .expect("ided: failed to write disk scheme");
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        } else {
            error!("Unknown event {}", event.fd);
        }

        // Handle todos to start new packets if possible
        let mut i = 0;
        while i < todo.len() {
            if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                socket_fd
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("ided: failed to write disk scheme");
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
                .expect("ided: failed to write disk scheme");
        }
    }

    std::process::exit(0);
}
