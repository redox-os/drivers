extern crate bitflags;
extern crate event;
extern crate spin;
extern crate syscall;

use redox_scheme::Socket;
use redox_scheme::wrappers::ReadinessBased;
use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::usize;

use event::{user_data, EventQueue};
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::allocate_first_msi_interrupt_on_bsp;
use pcid_interface::PciFunctionHandle;

pub mod hda;

/*
VEND:PROD
Virtualbox   8086:2668
QEMU ICH9    8086:293E
82801H ICH8  8086:284B
*/

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> File {
    let pci_config = pcid_handle.config();

    let all_pci_features = pcid_handle.fetch_all_features();
    log::debug!("PCI FEATURES: {:?}", all_pci_features);

    let has_msi = all_pci_features.iter().any(|feature| feature.is_msi());

    if has_msi {
        allocate_first_msi_interrupt_on_bsp(pcid_handle)
    } else if let Some(irq) = pci_config.func.legacy_interrupt_line {
        log::debug!("Legacy IRQ {}", irq);

        // legacy INTx# interrupt pins.
        irq.irq_handle("ihdad")
    } else {
        panic!("ihdad: no interrupts supported at all")
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> File {
    let pci_config = pcid_handle.config();

    if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        irq.irq_handle("ihdad")
    } else {
        panic!("ihdad: no interrupts supported at all")
    }
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ihda");

    common::setup_logging(
        "audio",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    log::info!("IHDA {}", pci_config.func.display());

    let address = unsafe { pcid_handle.map_bar(0) }.ptr.as_ptr() as usize;

    //TODO: MSI-X
    let mut irq_file = get_int_method(&mut pcid_handle);

    {
        let vend_prod: u32 = ((pci_config.func.full_device_id.vendor_id as u32) << 16)
            | (pci_config.func.full_device_id.device_id as u32);

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let event_queue =
            EventQueue::<Source>::new().expect("ihdad: Could not create event queue.");
        let device = RefCell::new(unsafe {
            hda::IntelHDA::new(address, vend_prod).expect("ihdad: failed to allocate device")
        });
        let socket = Socket::nonblock("audiohw").expect("ihdad: failed to create socket");
        let mut readiness_based = ReadinessBased::new(&socket, 16);

        event_queue
            .subscribe(socket.inner().raw(), Source::Scheme, event::EventFlags::READ)
            .unwrap();
        event_queue
            .subscribe(
                irq_file.as_raw_fd() as usize,
                Source::Irq,
                event::EventFlags::READ,
            )
            .unwrap();

        daemon.ready().expect("ihdad: failed to signal readiness");

        libredox::call::setrens(0, 0).expect("ihdad: failed to enter null namespace");

        let all = [Source::Irq, Source::Scheme];

        for event in all
            .into_iter()
            .chain(event_queue.map(|e| e.expect("failed to get next event").user_data))
        {
            match event {
                Source::Irq => {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();

                    if !device.borrow_mut().irq() {
                        continue;
                    }
                    irq_file.write(&mut irq).unwrap();

                    readiness_based.poll_all_requests(|| device.borrow_mut()).expect("ihdad: failed to poll requests");

                    /*
                    let next_read = device_irq.next_read();
                    if next_read > 0 {
                    return Ok(Some(next_read));
                    }
                    */
                }
                Source::Scheme => {
                    if !readiness_based.read_requests().expect("ihdad: failed to read from socket") {
                        break;
                    }
                    readiness_based.process_requests(|| device.borrow_mut());
                    if !readiness_based.write_responses().expect("ihdad: failed to write to socket") {
                        break;
                    }

                    /*
                    let next_read = device.borrow().next_read();
                    if next_read > 0 {
                    return Ok(Some(next_read));
                    }
                    */
                }
            }
        }

        std::process::exit(0);
    }
}

fn main() {
    // Daemonize
    redox_daemon::Daemon::new(daemon).expect("ihdad: failed to daemonize");
}
