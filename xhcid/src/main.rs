#![feature(renamed_spin_loop)]

use std::convert::TryFrom;
use std::fs::File;
use std::io::{self, prelude::*};
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::{Arc, Mutex};
use std::{mem, slice, thread};

use syscall::data::Packet;
use syscall::flag::{CloneFlags, O_RDWR, O_CLOEXEC, EVENT_READ};
use syscall::io::Io;
use syscall::io_uring::v1::Priority;

use pcid_interface::{MsiSetCapabilityInfo, MsiSetCapabilityInfoFlags, MsiXSetCapabilityInfo, MsiXSetCapabilityInfoFlags, PcidServerHandle, PciFunction, SetCapabilityInfo, PciBar};
use pcid_interface::msi::MsixTableEntry;
use pcid_interface::helpers::{self as pci_helpers, irq as irq_helpers};
use pci_helpers::{AllocatedBars, Bar};
use redox_iou::instance::ConsumerInstanceBuilder;

use futures::{SinkExt, StreamExt};
use futures::task::SpawnExt;
use log::info;
use redox_log::{RedoxLogger, OutputBuilder};

use crate::xhci::{InterruptMethod, InterruptSources, ForceSendFuture, Xhci};

// Declare as pub so that no warnings appear due to parts of the interface code not being used by
// the driver. Since there's also a dedicated crate for the driver interface, those warnings don't
// mean anything.
#[allow(dead_code)]
pub mod driver_interface;

mod usb;
mod xhci;

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_process_name("xhcid".into())
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Debug) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        )
        .with_output(
            OutputBuilder::with_endpoint(std::fs::OpenOptions::new()
                .create_new(false)
                .read(false)
                .write(true)
                .open("debug:").unwrap()
            )
                .with_ansi_escape_codes()
                //.with_filter(log::LevelFilter::Trace)
                .with_filter(log::LevelFilter::Debug)
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "host", "xhci.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Debug)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create xhci.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "host", "xhci.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Debug)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create xhci.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("xhcid: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("xhcid: failed to set default logger: {}", error);
            None
        }
    }
}

async fn get_int_method(func: &PciFunction, pcid_handle: &mut PcidServerHandle, allocated_bars: &AllocatedBars) -> (InterruptMethod, Option<InterruptSources>) {
    let all_pci_caps = pcid_handle.fetch_all_capabilities(Priority::default()).await.expect("xhcid: failed to fetch pci capabilities");
    info!("XHCI PCI FEATURES: {:?}", all_pci_caps);

    let msi_cap = all_pci_caps.iter().find_map(|cap| cap.as_pci()?.as_msi());
    let msix_cap = all_pci_caps.iter().find_map(|cap| cap.as_pci()?.as_msix());

    if let Some(capability) = msix_cap.cloned() {
        let (table_entries, pba_entries) = unsafe { irq_helpers::msix_cfg(func, &capability, allocated_bars).unwrap() };

        let mut info = xhci::MsixCfg {
            table: table_entries,
            pba: pba_entries,
            capability,
        };

        // Allocate one msi vector.
        use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

        assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
        let table_entry_pointer = &mut info.table[0];

        let destination_id = irq_helpers::read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
        let lapic_id = u8::try_from(destination_id).expect("xhcid: CPU id couldn't fit inside u8");
        let rh = false;
        let dm = false;
        let addr = x86_64_msix::message_address(lapic_id, rh, dm);

        let (vector, interrupt_handle) = irq_helpers::allocate_single_interrupt_vector(destination_id).expect("xhcid: failed to allocate interrupt vector").expect("xhcid: no interrupt vectors left");
        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

        table_entry_pointer.addr_lo.write(addr);
        table_entry_pointer.addr_hi.write(0);
        table_entry_pointer.msg_data.write(msg_data);
        table_entry_pointer.vec_ctl.writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);


        pcid_handle.set_capability(SetCapabilityInfo::MsiX(MsiXSetCapabilityInfo {
            flags: MsiXSetCapabilityInfoFlags::all().bits(),
            enabled: true.into(),
            function_mask: false.into(),
        }), Priority::default()).await.expect("xhcid: failed to enable MSI-X");

        // update our local mirror
        info.capability.set_msix_enabled(true);
        info.capability.set_function_mask(false);

        info!("Enabled MSI-X");

        (InterruptMethod::MsiX(Mutex::new(info)), Some(InterruptSources::MsiX(std::iter::once((0, interrupt_handle)).collect())))
    } else if let Some(capability) = msi_cap.cloned() {
        use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = irq_helpers::read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
        let lapic_id = u8::try_from(destination_id).expect("CPU id didn't fit inside u8");
        let msg_addr = x86_64_msix::message_address(lapic_id, false, false);

        let (vector, interrupt_handle) = irq_helpers::allocate_single_interrupt_vector(destination_id).expect("xhcid: failed to allocate interrupt vector").expect("xhcid: no interrupt vectors left");
        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

        let set_cap_info = MsiSetCapabilityInfo {
            flags: (MsiSetCapabilityInfoFlags::ENABLED | MsiSetCapabilityInfoFlags::MULTI_MESSAGE_ENABLE | MsiSetCapabilityInfoFlags::MESSAGE_ADDRESS | MsiSetCapabilityInfoFlags::MESSAGE_UPPER_ADDRESS | MsiSetCapabilityInfoFlags::MESSAGE_DATA).bits(),
            enabled: true.into(),
            multi_message_enable: 0,
            message_address: msg_addr,
            message_upper_address: 0,
            message_data: msg_data as u16,
            mask_bits: 0, // omitted due to lack of flag
        };
        pcid_handle.set_capability(SetCapabilityInfo::Msi(set_cap_info), Priority::default()).await.expect("xhcid: failed to set capability");
        info!("Enabled MSI");

        (InterruptMethod::Msi(Mutex::new(capability)), Some(InterruptSources::Msi(vec!(interrupt_handle))))
    } else if func.legacy_interrupt_pin().is_some() {
        // legacy INTx# interrupt pins.
        (InterruptMethod::Intx, Some(InterruptSources::Intx(File::open(format!("irq:{}", func.legacy_interrupt_line)).expect("xhcid: failed to open legacy IRQ file"))))
    } else {
        // no interrupts at all
        (InterruptMethod::Polling, None)
    }
}

fn main() {
    let _logger_ref = setup_logging();

    let address = std::env::args().nth(1).expect("expected address of PCI device for xhcid");

    let main_instance = ConsumerInstanceBuilder::new()
        .with_submission_entry_count(64)   // much smaller, only a single page (4k)
        .with_completion_entry_count(256)  // two pages (8k)
        .create_instance()
        .expect("xhcid: failed to create event queue io_uring instance")
        .map_all()
        .expect("xhcid: failed to map event io_uring buffers")
        .attach_to_kernel()
        .expect("xhcid: failed to attach event queue to kernel");

    let reactor = redox_iou::reactor::ReactorBuilder::new()
        .with_primary_instance(main_instance);
    let reactor = unsafe { reactor.assume_trusted_instance() };
    let reactor = reactor.build();

    let executor = redox_iou::executor::Executor::with_reactor(Arc::clone(&reactor));

    log::debug!("About to connect,..");
    let mut pcid_handle = executor.run(PcidServerHandle::connect_using_iouring(reactor.handle(), &address)).expect("xhcid: failed to setup channel to pcid");
    log::debug!("Connected");
    log::debug!("Fetching config...");
    let pci_config = executor.run(pcid_handle.fetch_config(Priority::default())).expect("xhcid: failed to fetch config");
    log::info!("XHCI PCI CONFIG: {:?}", pci_config);

    let bar = PciBar::parse_00_header_bars(pci_config.func.bars).unwrap()[0];
    log::info!("XHCI BAR: {}", bar.unwrap());

    let mut name = pci_config.func.scheme_friendly_name();
    name.push_str("_xhci");

    let allocated_bars = Arc::new(AllocatedBars::default());

    let bar_ptr = match bar {
        // TODO: Select memory type based on the uncachable bit, when physmapping (or
        // physallocating, when using MTRRs).
        Some(pcid_interface::PciBar::MemorySpace32 { address, .. }) => u64::from(address),
        Some(pcid_interface::PciBar::MemorySpace64 { address, .. }) => address,
        other => panic!("Expected memory bar, found {:?}", other),
    };
    let bar_size = usize::try_from(pci_config.func.bar_sizes[0]).expect("xhcid: bar size larger than usize");

    log::info!("XHCI BAR ADDRESS: {:#>08x}", bar_ptr);

    let bar_wrapper = unsafe { Bar::map(bar_ptr as usize, bar_size).expect("xhcid: failed to map BAR 0") };
    let address = bar_wrapper.pointer().as_ptr() as usize;

    log::info!("XHCI VIRT {:p} => {:p}", address as *const u8, unsafe { syscall::virttophys(address).unwrap() } as *const u8);
    log::info!("XHCI VIRT {:p} => {:p}", (address + 4096) as *const u8, unsafe { syscall::virttophys(address + 4096).unwrap() } as *const u8);
    log::info!("XHCI VIRT {:p} => {:p}", (address + 8192) as *const u8, unsafe { syscall::virttophys(address + 8192).unwrap() } as *const u8);
    log::info!("XHCI VIRT {:p} => {:p}", (address + 12288) as *const u8, unsafe { syscall::virttophys(address + 12288).unwrap() } as *const u8);

    *allocated_bars.0[0].lock().unwrap() = Some(bar_wrapper);

    let socket_fd = syscall::open(
        format!(":usb/{}", name),
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_CLOEXEC,
    )
    .expect("xhcid: failed to create usb scheme");
    let socket = Arc::new(unsafe {
        File::from_raw_fd(socket_fd as RawFd)
    });

    let (interrupt_method, interrupt_sources) = executor.run(get_int_method(&pci_config.func, &mut pcid_handle, &*allocated_bars));

    let event_queue_fd = syscall::open("event:", O_RDWR | O_CLOEXEC).expect("xhcid: failed to create main event queue");
    let mut event_queue = unsafe { File::from_raw_fd(event_queue_fd as RawFd) };

    let hci = Arc::new(Xhci::new(name, address, interrupt_method, pcid_handle, event_queue.try_clone().expect("xhcid: failed to dup event queue")).expect("xhcid: failed to allocate device"));
    xhci::start_irq_reactor(&hci, interrupt_sources);
    // TODO: Use the redox-iou executor once io_uring self-notification is implemented.
    futures::executor::block_on(hci.probe()).expect("xhcid: failed to probe");

    let (mut packet_sender, mut packet_receiver) = futures::channel::mpsc::channel(256);

    let socket_clone = Arc::clone(&socket);
    let packet_handler_thread = thread::Builder::new()
        .name("packet_handler".into())
        .spawn(move || {
        if event_queue.write(&syscall::Event {
            id: socket_fd,
            flags: EVENT_READ,
            data: 0,
        }).expect("xhcid: failed to register scheme socket for event queue") == 0 {
            panic!("xhcid: write 0 bytes when registering socket_fd");
        }

        let mut events = [syscall::Event::default(); 16];
        let mut packets = [Packet::default(); 16];

        'main_loop: loop {
            let events_buf = unsafe { slice::from_raw_parts_mut(events.as_mut_ptr() as *mut u8, events.len() * mem::size_of::<syscall::Event>()) };
            let byte_count = event_queue.read(events_buf).expect("xhcid: failed to read events");
            let count = byte_count / mem::size_of::<syscall::Event>();
            let events = &events[..count];

            for event in events {
                if event.id != socket_fd || event.data != 0 {
                    panic!("xhcid: got invalid main scheme socket event: {:?}", event);
                }
            }
            'packet_loop: loop {
                let packet_buf = unsafe { slice::from_raw_parts_mut(packets.as_mut_ptr() as *mut u8, packets.len() * mem::size_of::<Packet>()) };
                let byte_count = match (&*socket).read(packet_buf) {
                    // scheme is dead, someone unmounted `usb/*:`
                    Ok(0) => break 'main_loop,
                    Ok(b) => b,
                    Err(err) => match err.kind() {
                        io::ErrorKind::WouldBlock => break 'packet_loop,
                        _ => panic!("xhcid: failed to read from scheme socket: {}", err),
                    },
                };
                let count = byte_count / mem::size_of::<Packet>();
                let packets = &packets[..count];

                // TODO: Maybe process the packet until it can't make more progress at least once,
                // to improve latency for simpler requests (like reading an already finished
                // buffer).

                futures::executor::block_on(async {
                    packet_sender.send_all(&mut futures::stream::iter(packets.iter().copied().map(Ok))).await.expect("xhcid: failed to send packets to main thread");
                });
            }
        }
    }).expect("xhcid: failed to spawn packet handler thread");

    // TODO: Use a thread pool once the actual driver has been tested for correctness a bit more,
    // regarding driver concurrency.
    // TODO: Try getting the more fundamental futures to be Send, for example the command/transfer
    // submission and completion futures. Otherwise some futures may be wrapped in a struct and
    // force-implemented to be Send (if this even works).

    let mut executor = futures::executor::ThreadPoolBuilder::new()
        .name_prefix("runtime_thread_pool")
        .pool_size(1) // TODO
        .create().expect("xhcid: failed to create thread pool");

    syscall::setrens(0, 0).expect("xhcid: failed to enter null namespace");

    let context = xhci::SchemeIfCtx {
        hci: Arc::clone(&hci),
        socket_fd: Arc::clone(&socket_clone),
        spawner: executor.clone(),
    };

    executor.spawn(async move {
        while let Some(mut packet) = packet_receiver.next().await {
            let context_clone = context.clone();
            context.spawner.spawn(unsafe { ForceSendFuture::new(async move {
                context_clone.hci.scheme_handle(&mut packet, &context_clone).await;
                let _ = (&*context_clone.socket_fd).write(&packet).expect("xhcid: failed to write processed packet");
            }) }).expect("xhcid: failed to spawn packet handler");
        }
    });
    packet_handler_thread.join().expect("xhcid: failed to join packet handler thread");
}
