#![cfg_attr(target_arch = "aarch64", feature(stdsimd))] // Required for yield instruction
#![feature(int_roundings)]

extern crate syscall;
extern crate byteorder;

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::usize;

use pcid_interface::PcidServerHandle;
use syscall::error::{Error, ENODEV};
use syscall::data::{Event, Packet};
use syscall::flag::EVENT_READ;
use syscall::scheme::SchemeBlockMut;

use log::{error, info};
use redox_log::{OutputBuilder, RedoxLogger};

use crate::scheme::DiskScheme;

pub mod ahci;
pub mod scheme;

fn setup_logging(name: &str) -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", &format!("{}.log", name)) {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("ahcid: failed to create log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", &format!("{}.ansi.log", name)) {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("ahcid: failed to create ansi log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("ahcid: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("ahcid: failed to set default logger: {}", error);
            None
        }
    }
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("ahcid: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("ahcid: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("ahcid: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_ahci");

    let (bar, bar_size) = pci_config.func.bars[5].expect_mem();

    let irq = pci_config.func.legacy_interrupt_line;

    let _logger_ref = setup_logging(&name);

    info!(" + AHCI {} on: {} size: {} IRQ: {}", name, bar, bar_size, irq);

    let address = unsafe {
        common::physmap(
            bar,
            bar_size,
            common::Prot { read: true, write: true },
            common::MemoryType::Uncacheable,
        ).expect("ahcid: failed to map address")
    };
    {
        let scheme_name = format!("disk.{}", name);
        let socket_fd = syscall::open(
            &format!(":{}", scheme_name),
            syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK
        ).expect("ahcid: failed to create disk scheme");
        let mut socket = unsafe { File::from_raw_fd(socket_fd as RawFd) };

        let irq_fd = syscall::open(
            &format!("irq:{}", irq),
            syscall::O_RDWR | syscall::O_NONBLOCK
        ).expect("ahcid: failed to open irq file");
        let mut irq_file = unsafe { File::from_raw_fd(irq_fd as RawFd) };

        let mut event_file = File::open("event:").expect("ahcid: failed to open event file");

        syscall::setrens(0, 0).expect("ahcid: failed to enter null namespace");

        daemon.ready().expect("ahcid: failed to notify parent");

        event_file.write(&Event {
            id: socket_fd,
            flags: EVENT_READ,
            data: 0
        }).expect("ahcid: failed to event disk scheme");

        event_file.write(&Event {
            id: irq_fd,
            flags: EVENT_READ,
            data: 0
        }).expect("ahcid: failed to event irq scheme");

        let (hba_mem, disks) = ahci::disks(address as usize, &name);
        let mut scheme = DiskScheme::new(scheme_name, hba_mem, disks);

        let mut mounted = true;
        let mut todo = Vec::new();
        while mounted {
            let mut event = Event::default();
            if event_file.read(&mut event).expect("ahcid: failed to read event file") == 0 {
                break;
            }
            if event.id == socket_fd {
                loop {
                    let mut packet = Packet::default();
                    match socket.read(&mut packet) {
                        Ok(0) => {
                            mounted = false;
                            break;
                        },
                        Ok(_) => (),
                        Err(err) => if err.kind() == ErrorKind::WouldBlock {
                            break;
                        } else {
                            panic!("ahcid: failed to read disk scheme: {}", err);
                        }
                    }

                    if let Some(a) = scheme.handle(&packet) {
                        packet.a = a;
                        socket.write(&mut packet).expect("ahcid: failed to write disk scheme");
                    } else {
                        todo.push(packet);
                    }
                }
            } else if event.id == irq_fd {
                let mut irq = [0; 8];
                if irq_file.read(&mut irq).expect("ahcid: failed to read irq file") >= irq.len() {
                    if scheme.irq() {
                        irq_file.write(&irq).expect("ahcid: failed to write irq file");

                        // Handle todos in order to finish previous packets if possible
                        let mut i = 0;
                        while i < todo.len() {
                            if let Some(a) = scheme.handle(&todo[i]) {
                                let mut packet = todo.remove(i);
                                packet.a = a;
                                socket.write(&mut packet).expect("ahcid: failed to write disk scheme");
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
            } else {
                error!("Unknown event {}", event.id);
            }

            // Handle todos to start new packets if possible
            let mut i = 0;
            while i < todo.len() {
                if let Some(a) = scheme.handle(&todo[i]) {
                    let mut packet = todo.remove(i);
                    packet.a = a;
                    socket.write(&packet).expect("ahcid: failed to write disk scheme");
                } else {
                    i += 1;
                }
            }

            if ! mounted {
                for mut packet in todo.drain(..) {
                    packet.a = Error::mux(Err(Error::new(ENODEV)));
                    socket.write(&packet).expect("ahcid: failed to write disk scheme");
                }
            }
        }
    }

    std::process::exit(0);
}
