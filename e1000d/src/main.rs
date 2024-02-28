use std::cell::RefCell;
use std::convert::Infallible;
use std::io::{Read, Result, Write};
use std::os::unix::io::AsRawFd;
use std::rc::Rc;

use driver_network::NetworkScheme;
use event::EventQueue;
use pcid_interface::PcidServerHandle;
use syscall::EventFlags;

pub mod device;

fn main() {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("e1000d: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("e1000d: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_e1000");

    let bar = &pci_config.func.bars[0];

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("e1000d: no legacy interrupts supported");

    eprintln!(" + E1000 {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("e1000d");

        let address = unsafe { bar.physmap_mem("e1000d") } as usize;

        let device =
            unsafe { device::Intel8254x::new(address).expect("e1000d: failed to allocate device") };

        let scheme = Rc::new(RefCell::new(NetworkScheme::new(device, "network")));

        let mut event_queue =
            EventQueue::<Infallible>::new().expect("e1000d: failed to create event queue");

        syscall::setrens(0, 0).expect("e1000d: failed to enter null namespace");

        daemon
            .ready()
            .expect("e1000d: failed to mark daemon as ready");

        let scheme_irq = scheme.clone();
        event_queue
            .add(
                irq_file.as_raw_fd(),
                move |_event| -> Result<Option<Infallible>> {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq)?;
                    if unsafe { scheme_irq.borrow().adapter().irq() } {
                        irq_file.write(&mut irq)?;

                        return scheme_irq.borrow_mut().tick().map(|()| None);
                    }
                    Ok(None)
                },
            )
            .expect("e1000d: failed to catch events on IRQ file");

        let scheme_packet = scheme.clone();
        event_queue
            .add(
                scheme.borrow().event_handle(),
                move |_event| -> Result<Option<Infallible>> {
                    scheme_packet.borrow_mut().tick().map(|()| None)
                },
            )
            .expect("e1000d: failed to catch events on scheme file");

        event_queue
            .trigger_all(event::Event {
                fd: 0,
                flags: EventFlags::empty(),
            })
            .expect("e1000d: failed to trigger events");

        #[allow(unreachable_code)]
        match event_queue.run().expect("e1000d: failed to handle events") {}
    })
    .expect("e1000d: failed to create daemon");
}
