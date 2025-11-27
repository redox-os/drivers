use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
use pcid_interface::PciFunctionHandle;

pub mod device;
#[rustfmt::skip]
mod ixgbe;

fn main() {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ixgbe");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ixgbed: no legacy interrupts supported");

    println!(" + IXGBE {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("ixgbed");

        let mapped_bar = unsafe { pcid_handle.map_bar(0) };
        let address = mapped_bar.ptr.as_ptr();
        let size = mapped_bar.bar_size;

        let mut scheme = NetworkScheme::new(
            move || {
                device::Intel8259x::new(address as usize, size)
                    .expect("ixgbed: failed to allocate device")
            },
            daemon,
            format!("network.{name}"),
        );

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let event_queue =
            EventQueue::<Source>::new().expect("ixgbed: Could not create event queue.");
        event_queue
            .subscribe(
                irq_file.as_raw_fd() as usize,
                Source::Irq,
                event::EventFlags::READ,
            )
            .unwrap();
        event_queue
            .subscribe(
                scheme.event_handle().raw(),
                Source::Scheme,
                event::EventFlags::READ,
            )
            .unwrap();

        libredox::call::setrens(0, 0).expect("ixgbed: failed to enter null namespace");

        scheme.tick().unwrap();

        for event in event_queue.map(|e| e.expect("ixgbed: failed to get next event")) {
            match event.user_data {
                Source::Irq => {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();
                    if scheme.adapter().irq() {
                        irq_file.write(&mut irq).unwrap();

                        scheme.tick().unwrap();
                    }
                }
                Source::Scheme => {
                    scheme.tick().unwrap();
                }
            }
        }
        unreachable!()
    })
    .expect("ixgbed: failed to create daemon");
}
