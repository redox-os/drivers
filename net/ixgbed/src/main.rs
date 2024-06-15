use std::cell::RefCell;
use std::convert::Infallible;
use std::io::{Read, Result, Write};
use std::os::unix::io::AsRawFd;
use std::rc::Rc;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
use pcid_interface::PciFunctionHandle;
use syscall::EventFlags;

pub mod device;
#[rustfmt::skip]
mod ixgbe;

fn main() {
    let mut pcid_handle =
        PciFunctionHandle::connect_default().expect("ixgbed: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("ixgbed: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_ixgbe");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ixgbed: no legacy interrupts supported");

    println!(" + IXGBE {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("ixgbed");

        let (address, size) = unsafe {
            pci_config.func.bars[0].physmap_mem("ixgbed")
        };

        let device = device::Intel8259x::new(address as usize, size)
            .expect("ixgbed: failed to allocate device");

        let mut scheme = NetworkScheme::new(device, format!("network.{name}"));

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let mut event_queue = EventQueue::<Source>::new().expect("ixgbed: Could not create event queue.");
        event_queue.subscribe(irq_file.as_raw_fd() as usize, Source::Irq, event::EventFlags::READ).unwrap();
        event_queue.subscribe(scheme.event_handle() as usize, Source::Scheme, event::EventFlags::READ).unwrap();

        libredox::call::setrens(0, 0).expect("ixgbed: failed to enter null namespace");

        daemon
            .ready()
            .expect("ixgbed: failed to mark daemon as ready");

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
