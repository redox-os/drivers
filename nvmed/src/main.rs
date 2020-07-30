use std::convert::TryInto;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::{slice, usize};

use syscall::io_uring::v1::Priority;

use pcid_interface::{PciBar, PciFunction, PcidServerHandle};
use pcid_interface::helpers::{irq as irq_helpers, Bar, AllocatedBars};

pub use irq_helpers::InterruptSources;

use syscall::{
    CloneFlags, Event, Mmio, Packet, Result, SchemeBlockMut, PHYSMAP_NO_CACHE,
    PHYSMAP_WRITE,
};
use redox_log::{OutputBuilder, RedoxLogger};

use self::nvme::{InterruptMethod, Nvme};
use self::scheme::DiskScheme;

mod nvme;
mod scheme;

/// Get the most optimal yet functional interrupt mechanism: either (in the order of preference):
/// MSI-X, MSI, and INTx# pin. Returns both runtime interrupt structures (MSI/MSI-X capability
/// structures), and the handles to the interrupts.
async fn get_int_method(
    pcid_handle: &mut PcidServerHandle,
    function: &PciFunction,
    allocated_bars: &AllocatedBars,
) -> Result<(InterruptMethod, InterruptSources)> {
    log::trace!("Begin get_int_method");

    let capabilities = pcid_handle.fetch_all_capabilities(Priority::default()).await.expect("nvmed: failed to fetch all PCI(e) capabilities from pcid");

    // Cloning here is cheap because NVME doesn't use any function specific caps.
    let msi_cap = capabilities.iter().find_map(|cap| cap.as_pci()?.as_msi()).cloned();
    let msix_cap = capabilities.iter().find_map(|cap| cap.as_pci()?.as_msix()).cloned();

    // TODO: Allocate more than one vector when possible and useful.
    if let Some(mut capability_struct) = msix_cap {
        // Extended message signaled interrupts.
        use self::nvme::MsixCfg;
        use pcid_interface::msi::MsixTableEntry;

        let (table_entries, pba_entries) = unsafe { irq_helpers::msix_cfg(function, &capability_struct, allocated_bars).unwrap() };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for table_entry in table_entries.iter_mut() {
            table_entry.mask();
        }

        pcid_handle.set_capability(pcid_interface::SetCapabilityInfo::MsiX(pcid_interface::MsiXSetCapabilityInfo {
            flags: pcid_interface::MsiXSetCapabilityInfoFlags::all().bits(),
            enabled: true.into(),
            function_mask: false.into(),
        }), Priority::default());
        capability_struct.set_msix_enabled(true); // only affects our local mirror of the cap

        let (msix_vector_number, irq_handle) = {
            use msi_x86_64::DeliveryMode;
            use pcid_interface::msi::x86_64 as msi_x86_64;

            let entry: &mut MsixTableEntry = &mut table_entries[0];

            let bsp_cpu_id =
                irq_helpers::read_bsp_apic_id().expect("nvmed: failed to read APIC ID");
            let bsp_lapic_id = bsp_cpu_id
                .try_into()
                .expect("nvmed: BSP local apic ID couldn't fit inside u8");
            let (vector, irq_handle) = irq_helpers::allocate_single_interrupt_vector(bsp_cpu_id)
                .expect("nvmed: failed to allocate single MSI-X interrupt vector")
                .expect("nvmed: no interrupt vectors left on BSP");

            let msg_addr = msi_x86_64::message_address(bsp_lapic_id, false, false);
            let msg_data = msi_x86_64::message_data_edge_triggered(DeliveryMode::Fixed, vector);

            entry.set_addr_lo(msg_addr);
            entry.set_msg_data(msg_data);

            (0, irq_handle)
        };

        let interrupt_method = InterruptMethod::MsiX(MsixCfg {
            cap: capability_struct,
            table: table_entries,
            pba: pba_entries,
        });
        let interrupt_sources =
            InterruptSources::MsiX(std::iter::once((msix_vector_number, irq_handle)).collect());

        Ok((interrupt_method, interrupt_sources))
    } else if let Some(capability_struct) = msi_cap {
        // Message signaled interrupts.
        let irq_handle = {
            use msi_x86_64::DeliveryMode;
            use pcid_interface::msi::x86_64 as msi_x86_64;
            use pcid_interface::{MsiSetCapabilityInfo, MsiSetCapabilityInfoFlags, SetCapabilityInfo};

            let bsp_cpu_id =
                irq_helpers::read_bsp_apic_id().expect("nvmed: failed to read BSP APIC ID");
            let bsp_lapic_id = bsp_cpu_id
                .try_into()
                .expect("nvmed: BSP local apic ID couldn't fit inside u8");
            let (vector, irq_handle) = irq_helpers::allocate_single_interrupt_vector(bsp_cpu_id)
                .expect("nvmed: failed to allocate single MSI interrupt vector")
                .expect("nvmed: no interrupt vectors left on BSP");

            let msg_addr = msi_x86_64::message_address(bsp_lapic_id, false, false);
            let msg_data =
                msi_x86_64::message_data_edge_triggered(DeliveryMode::Fixed, vector) as u16;

            pcid_handle.set_capability(SetCapabilityInfo::Msi(MsiSetCapabilityInfo {
                flags: (MsiSetCapabilityInfoFlags::ENABLED | MsiSetCapabilityInfoFlags::MESSAGE_ADDRESS | MsiSetCapabilityInfoFlags::MESSAGE_UPPER_ADDRESS | MsiSetCapabilityInfoFlags::MESSAGE_DATA | MsiSetCapabilityInfoFlags::MULTI_MESSAGE_ENABLE).bits(),
                enabled: true.into(),
                message_address: msg_addr,
                message_upper_address: 0,
                message_data: msg_data,
                multi_message_enable: 0, // enable 2^0=1 vectors
                mask_bits: 0, // omitted due to lack of flag
            }), Priority::default()).await.expect("nvmed: failed to set MSI registers");

            irq_handle
        };

        let interrupt_method = InterruptMethod::Msi(capability_struct);
        let interrupt_sources =
            InterruptSources::Msi(std::iter::once(irq_handle).collect());

        Ok((interrupt_method, interrupt_sources))
    } else if function.legacy_interrupt_pin().is_some() {
        // INTx# pin based interrupts.
        let irq_handle = File::open(format!("irq:{}", function.legacy_interrupt_line))
            .expect("nvmed: failed to open INTx# interrupt line");
        Ok((InterruptMethod::Intx, InterruptSources::Intx(irq_handle)))
    } else {
        // No interrupts at all
        todo!("handling of no interrupts")
    }
}

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "nvme.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("nvmed: failed to create nvme.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "nvme.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("nvmed: failed to create nvme.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("nvmed: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("nvmed: failed to set default logger: {}", error);
            None
        }
    }
}

fn main() {
    // Daemonize
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    let _logger_ref = setup_logging();

    let mut pcid_handle =
        PcidServerHandle::connect_using_pipes_from_env_fds().expect("nvmed: failed to setup channel to pcid");
    let pci_config = futures::executor::block_on(pcid_handle
        .fetch_config(Priority::default()))
        .expect("nvmed: failed to fetch config");

    let bar = match pci_config.func.bars[0] {
        PciBar::MemorySpace32 { address, .. } => u64::from(address),
        PciBar::MemorySpace64 { address, .. } => address,
        other => panic!("received a non-memory BAR ({:?})", other),
    };
    let bar_size = pci_config.func.bar_sizes[0];

    let mut name = pci_config.func.name();
    name.push_str("_nvme");

    log::info!("NVME PCI CONFIG: {:?}", pci_config);

    let allocated_bars = AllocatedBars::default();
    let bar_wrapper = unsafe { Bar::map(bar as usize, bar_size as usize).unwrap() };
    let address = bar_wrapper.pointer().as_ptr() as usize;
    *allocated_bars.0[0].lock().unwrap() = Some(bar_wrapper);
    let event_fd = syscall::open("event:", syscall::O_RDWR | syscall::O_CLOEXEC)
        .expect("nvmed: failed to open event queue");
    let mut event_file = unsafe { File::from_raw_fd(event_fd as RawFd) };

    let scheme_name = format!("disk/{}", name);
    let socket_fd = syscall::open(
        &format!(":{}", scheme_name),
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK | syscall::O_CLOEXEC,
    )
    .expect("nvmed: failed to create disk scheme");

    syscall::write(
        event_fd,
        &syscall::Event {
            id: socket_fd,
            flags: syscall::EVENT_READ,
            data: 0,
        },
    )
    .expect("nvmed: failed to watch disk scheme events");

    let mut socket_file = unsafe { File::from_raw_fd(socket_fd as RawFd) };

    let (reactor_sender, reactor_receiver) = crossbeam_channel::unbounded();
    let (interrupt_method, interrupt_sources) =
        futures::executor::block_on(get_int_method(&mut pcid_handle, &pci_config.func, &allocated_bars))
            .expect("nvmed: failed to find a suitable interrupt method");
    let mut nvme = Nvme::new(address, interrupt_method, pcid_handle, reactor_sender)
        .expect("nvmed: failed to allocate driver data");
    unsafe { nvme.init() }
    log::debug!("Finished base initialization");
    let nvme = Arc::new(nvme);
    let reactor_thread = nvme::cq_reactor::start_cq_reactor_thread(Arc::clone(&nvme), interrupt_sources, reactor_receiver);
    let namespaces = futures::executor::block_on(nvme.init_with_queues());

    syscall::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    let mut scheme = DiskScheme::new(scheme_name, nvme, namespaces);
    let mut todo = Vec::new();
    'events: loop {
        let mut event = Event::default();
        if event_file
            .read(&mut event)
            .expect("nvmed: failed to read event queue")
            == 0
        {
            break;
        }

        match event.data {
            0 => loop {
                let mut packet = Packet::default();
                match socket_file.read(&mut packet) {
                    Ok(0) => break 'events,
                    Ok(_) => (),
                    Err(err) => match err.kind() {
                        ErrorKind::WouldBlock => break,
                        _ => Err(err).expect("nvmed: failed to read disk scheme"),
                    },
                }
                todo.push(packet);
            },
            unknown => {
                panic!("nvmed: unknown event data {}", unknown);
            }
        }

        let mut i = 0;
        while i < todo.len() {
            if let Some(a) = scheme.handle(&todo[i]) {
                let mut packet = todo.remove(i);
                packet.a = a;
                socket_file
                    .write(&packet)
                    .expect("nvmed: failed to write disk scheme");
            } else {
                i += 1;
            }
        }
    }

    //TODO: destroy NVMe stuff
    reactor_thread.join().expect("nvmed: failed to join reactor thread");
}
