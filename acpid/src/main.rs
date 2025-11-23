use std::convert::TryFrom;
use std::fs::File;
use std::mem;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use event::{EventFlags, RawEventQueue};
use redox_scheme::{RequestKind, SignalBehavior, Socket};
use syscall::{EAGAIN, EWOULDBLOCK};

mod acpi;
mod aml_physmem;

mod scheme;

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    common::setup_logging(
        "misc",
        "acpi",
        "acpid",
        common::output_level(),
        common::file_level(),
    );

    let rxsdt_raw_data: Arc<[u8]> = std::fs::read("/scheme/kernel.acpi/rxsdt")
        .expect("acpid: failed to read `/scheme/kernel.acpi/rxsdt`")
        .into();

    if rxsdt_raw_data.is_empty() {
        log::info!("System doesn't use ACPI");
        daemon.ready().expect("acpid: failed to notify parent");
        std::process::exit(0);
    }

    let sdt = self::acpi::Sdt::new(rxsdt_raw_data).expect("acpid: failed to parse [RX]SDT");

    let mut thirty_two_bit;
    let mut sixty_four_bit;

    let physaddrs_iter = match &sdt.signature {
        b"RSDT" => {
            thirty_two_bit = sdt
                .data()
                .chunks(mem::size_of::<u32>())
                // TODO: With const generics, the compiler has some way of doing this for static sizes.
                .map(|chunk| <[u8; mem::size_of::<u32>()]>::try_from(chunk).unwrap())
                .map(|chunk| u32::from_le_bytes(chunk))
                .map(u64::from);

            &mut thirty_two_bit as &mut dyn Iterator<Item = u64>
        }
        b"XSDT" => {
            sixty_four_bit = sdt
                .data()
                .chunks(mem::size_of::<u64>())
                .map(|chunk| <[u8; mem::size_of::<u64>()]>::try_from(chunk).unwrap())
                .map(|chunk| u64::from_le_bytes(chunk));

            &mut sixty_four_bit as &mut dyn Iterator<Item = u64>
        }
        _ => panic!("acpid: expected [RX]SDT from kernel to be either of those"),
    };

    let acpi_context = self::acpi::AcpiContext::init(physaddrs_iter);

    // TODO: I/O permission bitmap?
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    common::acquire_port_io_rights().expect("acpid: failed to set I/O privilege level to Ring 3");

    let shutdown_pipe = File::open("/scheme/kernel.acpi/kstop")
        .expect("acpid: failed to open `/scheme/kernel.acpi/kstop`");

    let mut event_queue = RawEventQueue::new().expect("acpid: failed to create event queue");
    let socket = Socket::nonblock("acpi").expect("acpid: failed to create disk scheme");

    daemon.ready().expect("acpid: failed to notify parent");

    //TODO: needs to open /scheme/pci/access later! libredox::call::setrens(0, 0).expect("acpid: failed to enter null namespace");

    event_queue
        .subscribe(shutdown_pipe.as_raw_fd() as usize, 0, EventFlags::READ)
        .expect("acpid: failed to register shutdown pipe for event queue");
    event_queue
        .subscribe(socket.inner().raw(), 1, EventFlags::READ)
        .expect("acpid: failed to register scheme socket for event queue");

    let mut scheme = self::scheme::AcpiScheme::new(&acpi_context);

    let mut mounted = true;
    while mounted {
        let Some(event) = event_queue
            .next()
            .transpose()
            .expect("acpid: failed to read event file")
        else {
            break;
        };

        if event.fd == socket.inner().raw() {
            loop {
                let req = match socket.next_request(SignalBehavior::Interrupt) {
                    Ok(None) => {
                        mounted = false;
                        break;
                    }
                    Ok(Some(req)) => req,
                    Err(err) => {
                        if err.errno == EWOULDBLOCK || err.errno == EAGAIN {
                            break;
                        } else {
                            panic!("acpid: failed to read next request: {}", err);
                        }
                    }
                };

                match req.kind() {
                    RequestKind::Call(call) => {
                        let response = call.handle_sync(&mut scheme);
                        socket
                            .write_response(response, SignalBehavior::Restart)
                            .expect("acpid: failed to write response");
                    }
                    RequestKind::OnClose { id } => {
                        scheme.on_close(id);
                    }
                    _ => (),
                }
            }
        } else if event.fd == shutdown_pipe.as_raw_fd() as usize {
            log::info!("Received shutdown request from kernel.");
            mounted = false;
        } else {
            log::debug!("Received request to unknown fd: {}", event.fd);
            continue;
        }
    }

    drop(shutdown_pipe);
    drop(event_queue);

    acpi_context.set_global_s_state(5);

    unreachable!("System should have shut down before this is entered");
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("acpid: failed to daemonize");
}
