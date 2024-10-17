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

fn main() {
    let mut pcid_handle =
        PciFunctionHandle::connect_default().expect("e1000d: failed to setup channel to pcid");
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_e1000");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("e1000d: no legacy interrupts supported");

    eprintln!(" + E1000 {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("e1000d");

        let address = unsafe { pcid_handle.map_bar(0) }
            .expect("e1000d")
            .ptr
            .as_ptr() as usize;

        let device =
            unsafe { device::Intel8254x::new(address).expect("e1000d: failed to allocate device") };

        let mut scheme = NetworkScheme::new(device, format!("network.{name}"));

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let mut event_queue =
            EventQueue::<Source>::new().expect("e1000d: failed to create event queue");

        event_queue
            .subscribe(
                irq_file.as_raw_fd() as usize,
                Source::Irq,
                event::EventFlags::READ,
            )
            .expect("e1000d: failed to subscribe to IRQ fd");
        event_queue
            .subscribe(
                scheme.event_handle() as usize,
                Source::Scheme,
                event::EventFlags::READ,
            )
            .expect("e1000d: failed to subscribe to scheme fd");

        libredox::call::setrens(0, 0).expect("e1000d: failed to enter null namespace");

        daemon
            .ready()
            .expect("e1000d: failed to mark daemon as ready");

        scheme.tick().unwrap();

        for event in event_queue.map(|e| e.expect("e1000d: failed to get event")) {
            match event.user_data {
                Source::Irq => {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();
                    if unsafe { scheme.adapter().irq() } {
                        irq_file.write(&mut irq).unwrap();

                        scheme.tick().expect("e1000d: failed to handle IRQ")
                    }
                }
                Source::Scheme => scheme.tick().expect("e1000d: failed to handle scheme op"),
            }
        }
        unreachable!()
    })
    .expect("e1000d: failed to create daemon");
}
