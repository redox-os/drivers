//#![deny(warnings)]

use libredox::{flag, Fd};
use redox_scheme::wrappers::ReadinessBased;
use redox_scheme::Socket;
use std::cell::RefCell;
use std::{env, usize};

use event::{user_data, EventQueue};

pub mod device;

fn main() {
    let mut args = env::args().skip(1);

    let addr_str = args.next().unwrap_or("220".to_string());
    let addr = u16::from_str_radix(&addr_str, 16).expect("sb16: failed to parse address");

    println!(" + sb16 at 0x{:X}\n", addr);

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        common::setup_logging(
            "audio",
            "pci",
            "sb16",
            common::output_level(),
            common::file_level(),
        );

        common::acquire_port_io_rights().expect("sb16d: failed to acquire port IO rights");

        let device = RefCell::new(unsafe {
            device::Sb16::new(addr).expect("sb16d: failed to allocate device")
        });
        let socket = Socket::nonblock("audiohw").expect("sb16d: failed to create socket");
        let mut readiness_based = ReadinessBased::new(&socket, 16);

        //TODO: error on multiple IRQs?
        let irq_file = match device.borrow().irqs.first() {
            Some(irq) => Fd::open(&format!("/scheme/irq/{}", irq), flag::O_RDWR, 0)
                .expect("sb16d: failed to open IRQ file"),
            None => panic!("sb16d: no IRQs found"),
        };
        user_data! {
            enum Source {
                Irq,
                Scheme,
            }
        }

        let event_queue =
            EventQueue::<Source>::new().expect("sb16d: Could not create event queue.");
        event_queue
            .subscribe(irq_file.raw(), Source::Irq, event::EventFlags::READ)
            .unwrap();
        event_queue
            .subscribe(
                socket.inner().raw(),
                Source::Scheme,
                event::EventFlags::READ,
            )
            .unwrap();

        daemon.ready().expect("sb16d: failed to signal readiness");

        libredox::call::setrens(0, 0).expect("sb16d: failed to enter null namespace");

        let all = [Source::Irq, Source::Scheme];

        for event in all
            .into_iter()
            .chain(event_queue.map(|e| e.expect("sb16d: failed to get next event").user_data))
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
                        .expect("sb16d: failed to poll requests");

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
                        .expect("sb16d: failed to read from socket")
                    {
                        break;
                    }
                    readiness_based.process_requests(|| device.borrow_mut());
                    if !readiness_based
                        .write_responses()
                        .expect("sb16d: failed to write to socket")
                    {
                        break;
                    }
                }
            }
        }

        std::process::exit(0);
    })
    .expect("sb16d: failed to daemonize");
}
