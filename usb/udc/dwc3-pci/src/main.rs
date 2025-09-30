pub mod pci;

use pcid_interface::PciFunctionHandle;
use dwc3::DWC3;
use driver_udc::{UDCAdapter, UDCScheme};
use event::{EventFlags, EventQueue};
use std::os::unix::io::AsRawFd;
use std::io::{Write,Read};

fn main() {
    common::setup_logging(
        "pci",
        "udc",
        "dwc3",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();
    
    let mut name = pci_config.func.name();
    name.push_str("_dwc3");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("dwc3-pci: no legacy interrupts supported");
    eprintln!(" + dwc3-pci {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("dwc3-pci");
        let irq_fd = irq_file.as_raw_fd() as usize;

        let address = unsafe { pcid_handle.map_bar(0) }.ptr.as_ptr() as usize;
        let device = DWC3::new(address).expect("dwc3-pci: failed to initialize the dwc3 module");
        let mut scheme = UDCScheme::new(device, format!("udc.{}", name));

        event::user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        };

        let event_queue = EventQueue::<Source>::new().expect("dwc3-pci: failed to create event queue");

        libredox::call::setrens(0, 0).expect("dwc3-pci: failed to enter null namespace");

        event_queue
            .subscribe(scheme.event_handle().raw(), Source::Scheme, EventFlags::READ)
            .expect("dwc3-pci: failed to event scheme socket");

        /*
        event_queue
            .subscribe(irq_fd, Source::Irq, EventFlags::READ)
            .expect("dwc3-pci: failed to event irq scheme");*/

        daemon
            .ready()
            .expect("dwc3-pci: failed to mark daemon as ready");

        scheme.tick().unwrap();

        for event in event_queue.map(|e| e.expect("dwc3-pci: failed to get event")) {
            match event.user_data {
                Source::Irq => {/*
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();
                    if unsafe { scheme.udc().irq() } {
                        irq_file.write(&mut irq).unwrap();

                        scheme.tick().expect("dwc3-pci: failed to handle IRQ")
                    }*/
                }
                Source::Scheme => scheme.tick().expect("dwc3-pci: failed to handle scheme op"),
            }
        }

        unreachable!();
    })
    .expect("dwc3-pci: failed to create daemon");
}
