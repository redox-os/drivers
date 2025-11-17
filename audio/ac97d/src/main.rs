//#![deny(warnings)]

extern crate bitflags;
extern crate event;
extern crate spin;
extern crate syscall;

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::usize;

use event::{user_data, EventQueue};
use pcid_interface::PciFunctionHandle;
use redox_scheme::wrappers::ReadinessBased;
use redox_scheme::Socket;
use std::cell::RefCell;

pub mod device;

fn main() {
    let pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_ac97");

    let bar0 = pci_config.func.bars[0].expect_port();
    let bar1 = pci_config.func.bars[1].expect_port();

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("ac97d: no legacy interrupts supported");

    println!(" + ac97 {}", pci_config.func.display());

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        common::setup_logging(
            "audio",
            "pci",
            &name,
            common::output_level(),
            common::file_level(),
        );

        common::acquire_port_io_rights()
            .expect("ac97d: failed to set I/O privilege level to Ring 3");

        let mut irq_file = irq.irq_handle("ac97d");

        let device = RefCell::new(unsafe {
            device::Ac97::new(bar0, bar1).expect("ac97d: failed to allocate device")
        });
        let socket = Socket::nonblock("audiohw").expect("ac97d: failed to create socket");
        let mut readiness_based = ReadinessBased::new(&socket, 16);

        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let event_queue =
            EventQueue::<Source>::new().expect("ac97d: Could not create event queue.");
        event_queue
            .subscribe(
                irq_file.as_raw_fd() as usize,
                Source::Irq,
                event::EventFlags::READ,
            )
            .unwrap();
        event_queue
            .subscribe(
                socket.inner().raw(),
                Source::Scheme,
                event::EventFlags::READ,
            )
            .unwrap();

        daemon.ready().expect("ac97d: failed to signal readiness");

        libredox::call::setrens(0, 0).expect("ac97d: failed to enter null namespace");

        let all = [Source::Irq, Source::Scheme];
        for event in all
            .into_iter()
            .chain(event_queue.map(|e| e.expect("ac97d: failed to get next event").user_data))
        {
            match event {
                Source::Irq => {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq).unwrap();

                    if !device.borrow_mut().irq() {
                        continue;
                    }
                    irq_file.write(&mut irq).unwrap();

                    readiness_based
                        .poll_all_requests(|| device.borrow_mut())
                        .expect("ac97d: failed to poll requests");

                    /*
                    let next_read = device_irq.next_read();
                    if next_read > 0 {
                    return Ok(Some(next_read));
                    }
                    */
                }
                Source::Scheme => {
                    if !readiness_based
                        .read_requests()
                        .expect("ac97d: failed to read from socket")
                    {
                        break;
                    }
                    readiness_based.process_requests(|| device.borrow_mut());
                    if !readiness_based
                        .write_responses()
                        .expect("ac97d: failed to write to socket")
                    {
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
    })
    .expect("ac97d: failed to daemonize");
}
