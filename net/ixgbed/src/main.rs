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
#[rustfmt::skip]
mod ixgbe;

const IXGBE_MMIO_SIZE: usize = 512 * 1024;

fn main() {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("ixgbed: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("ixgbed: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_ixgbe");

    let (bar, _) = pci_config.func.bars[0].expect_mem();

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ixgbed: no legacy interrupts supported");

    println!(" + IXGBE {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let mut irq_file = irq.irq_handle("ixgbed");

        let address = unsafe {
            common::physmap(
                bar,
                IXGBE_MMIO_SIZE,
                common::Prot::RW,
                common::MemoryType::Uncacheable,
            )
            .expect("ixgbed: failed to map address") as usize
        };

        let device = device::Intel8259x::new(address, IXGBE_MMIO_SIZE)
            .expect("ixgbed: failed to allocate device");

        let scheme = Rc::new(RefCell::new(NetworkScheme::new(device, format!("network.{name}"))));

        let mut event_queue =
            EventQueue::<Infallible>::new().expect("ixgbed: failed to create event queue");

        syscall::setrens(0, 0).expect("ixgbed: failed to enter null namespace");

        daemon
            .ready()
            .expect("ixgbed: failed to mark daemon as ready");

        let scheme_irq = scheme.clone();
        event_queue
            .add(
                irq_file.as_raw_fd(),
                move |_event| -> Result<Option<Infallible>> {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq)?;
                    if scheme_irq.borrow().adapter().irq() {
                        irq_file.write(&mut irq)?;

                        return scheme_irq.borrow_mut().tick().map(|()| None);
                    }
                    Ok(None)
                },
            )
            .expect("ixgbed: failed to catch events on IRQ file");

        let scheme_packet = scheme.clone();
        event_queue
            .add(
                scheme.borrow().event_handle(),
                move |_event| -> Result<Option<Infallible>> {
                    scheme_packet.borrow_mut().tick().map(|()| None)
                },
            )
            .expect("ixgbed: failed to catch events on scheme file");

        event_queue
            .trigger_all(event::Event {
                fd: 0,
                flags: EventFlags::empty(),
            })
            .expect("ixgbed: failed to trigger events");

        #[allow(unreachable_code)]
        match event_queue.run().expect("ixgbed: failed to handle events") {}
    })
    .expect("ixgbed: failed to create daemon");
}
