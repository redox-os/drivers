#![cfg_attr(target_arch = "aarch64", feature(stdsimd))] // Required for yield instruction
#![feature(int_roundings)]

extern crate syscall;
extern crate byteorder;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::usize;

use event::{EventFlags, RawEventQueue};
use pcid_interface::PcidServerHandle;
use redox_scheme::{RequestKind, Response, SignalBehavior, Socket, V2};
use syscall::error::{Error, ENODEV};

use log::{error, info};
use redox_log::{OutputBuilder, RedoxLogger};
use syscall::{EAGAIN, EWOULDBLOCK};

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

    let bar = &pci_config.func.bars[5];

    let irq = pci_config.func.legacy_interrupt_line.expect("ahcid: no legacy interrupts supported");

    let _logger_ref = setup_logging(&name);

    info!(" + AHCI {}", pci_config.func.display());

    let address = unsafe { bar.physmap_mem("ahcid") };
    {
        let scheme_name = format!("disk.{}", name);
        let socket = Socket::<V2>::nonblock(&scheme_name).expect("ahcid: failed to create disk scheme");

        let mut irq_file = irq.irq_handle("ahcid");
        let irq_fd = irq_file.as_raw_fd() as usize;

        let mut event_queue = RawEventQueue::new().expect("ahcid: failed to create event queue");

        libredox::call::setrens(0, 0).expect("ahcid: failed to enter null namespace");

        event_queue.subscribe(socket.inner().raw(), 1, EventFlags::READ).expect("ahcid: failed to event scheme socket");
        event_queue.subscribe(irq_fd, 1, EventFlags::READ).expect("ahcid: failed to event irq scheme");

        daemon.ready().expect("ahcid: failed to notify parent");

        let (hba_mem, disks) = ahci::disks(address as usize, &name);
        let mut scheme = DiskScheme::new(scheme_name, hba_mem, disks);

        let mut mounted = true;
        let mut todo = Vec::new();
        while mounted {
            let Some(event) = event_queue.next().transpose().expect("ahcid: failed to read event file") else {
                break;
            };
            if event.fd == socket.inner().raw() {
                loop {
                    let sqe = match socket.next_request(SignalBehavior::Interrupt) {
                        Ok(None) => {
                            mounted = false;
                            break;
                        },
                        Ok(Some(s)) => if let RequestKind::Call(call) = s.kind() {
                            call
                        } else {
                            // TODO: Support e.g. cancellation
                            continue;
                        },
                        Err(err) => if err.errno == EWOULDBLOCK || err.errno == EAGAIN {
                            break;
                        } else {
                            panic!("ahcid: failed to read disk scheme: {}", err);
                        }
                    };

                    if let Some(response) = sqe.handle_scheme_block_mut(&mut scheme) {
                        // TODO: handle full CQE?
                        socket.write_response(response, SignalBehavior::Restart).expect("ahcid: failed to write disk scheme");
                    } else {
                        todo.push(sqe);
                    }
                }
            } else if event.fd == irq_fd {
                let mut irq = [0; 8];
                if irq_file.read(&mut irq).expect("ahcid: failed to read irq file") >= irq.len() {
                    if scheme.irq() {
                        irq_file.write(&irq).expect("ahcid: failed to write irq file");

                        // Handle todos in order to finish previous packets if possible
                        let mut i = 0;
                        while i < todo.len() {
                            if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                                let _sqe = todo.remove(i);
                                socket.write_response(resp, SignalBehavior::Restart).expect("ahcid: failed to write disk scheme");
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
                if let Some(response) = todo[i].handle_scheme_block_mut(&mut scheme) {
                    let _sqe = todo.remove(i);
                    socket.write_response(response, SignalBehavior::Restart).expect("ahcid: failed to write disk scheme");
                } else {
                    i += 1;
                }
            }

            if ! mounted {
                for sqe in todo.drain(..) {
                    socket.write_response(Response::new(&sqe, Err(Error::new(ENODEV))), SignalBehavior::Restart).expect("ahcid: failed to write disk scheme");
                }
            }
        }
    }

    std::process::exit(0);
}
