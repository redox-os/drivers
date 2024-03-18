#[macro_use]
extern crate bitflags;

use std::convert::{TryFrom, TryInto};
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::env;

use libredox::flag;
use pcid_interface::{MsiSetFeatureInfo, PcidServerHandle, PciFeature, PciFeatureInfo, SetFeatureInfo};
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::allocate_single_interrupt_vector_for_msi;
use pcid_interface::irq_helpers::read_bsp_apic_id;
use pcid_interface::msi::MsixTableEntry;

use event::{Event, RawEventQueue};
use redox_log::{RedoxLogger, OutputBuilder};
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::flag::EventFlags;
use syscall::scheme::Scheme;
use syscall::io::Io;

use crate::xhci::{InterruptMethod, Xhci};

// Declare as pub so that no warnings appear due to parts of the interface code not being used by
// the driver. Since there's also a dedicated crate for the driver interface, those warnings don't
// mean anything.
pub mod driver_interface;

mod usb;
mod xhci;

async fn handle_packet(hci: Arc<Xhci>, packet: Packet) -> Packet {
    todo!()
}

fn setup_logging(name: &str) -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "host", &format!("{}.log", name)) {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Debug)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create {}.log: {}", name, error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "host", &format!("{}.ansi.log", name)) {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Debug)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create {}.ansi.log: {}", name, error),
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

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PcidServerHandle, bar0_address: usize) -> (Option<File>, InterruptMethod) {
    let pci_config = pcid_handle.fetch_config().expect("xhcid: failed to fetch config");

    let all_pci_features = pcid_handle.fetch_all_features().expect("xhcid: failed to fetch pci features");
    log::debug!("XHCI PCI FEATURES: {:?}", all_pci_features);

    let (has_msi, mut msi_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msi(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));
    let (has_msix, mut msix_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msix(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));

    if has_msi && !msi_enabled && !has_msix {
        msi_enabled = true;
    }
    if has_msix && !msix_enabled {
        msix_enabled = true;
    }

    if msi_enabled && !msix_enabled {
        let mut capability = match pcid_handle.feature_info(PciFeature::Msi).expect("xhcid: failed to retrieve the MSI capability structure from pcid") {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
        let (msg_addr_and_data, interrupt_handle) = allocate_single_interrupt_vector_for_msi(destination_id);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address_and_data: Some(msg_addr_and_data),
            mask_bits: None,
        };
        pcid_handle.set_feature_info(SetFeatureInfo::Msi(set_feature_info)).expect("xhcid: failed to set feature info");

        pcid_handle.enable_feature(PciFeature::Msi).expect("xhcid: failed to enable MSI");
        log::debug!("Enabled MSI");

        (Some(interrupt_handle), InterruptMethod::Msi)
    } else if msix_enabled {
        let capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("xhcid: failed to retrieve the MSI-X capability structure from pcid") {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        capability.validate(pci_config.func.bars);

        assert_eq!(capability.table_bir(), 0);
        let virt_table_base = (bar0_address + capability.table_offset() as usize) as *mut MsixTableEntry;

        let mut info = xhci::MsixInfo {
            virt_table_base: NonNull::new(virt_table_base).unwrap(),
            capability,
        };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) = allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            (Some(interrupt_handle), InterruptMethod::MsiX(Mutex::new(info)))
        };

        pcid_handle.enable_feature(PciFeature::MsiX).expect("xhcid: failed to enable MSI-X");
        log::debug!("Enabled MSI-X");

        method
    } else if let Some(irq) = pci_config.func.legacy_interrupt_line {
        log::debug!("Legacy IRQ {}", irq);

        // legacy INTx# interrupt pins.
        (Some(irq.irq_handle("xhcid")), InterruptMethod::Intx)
    } else {
        // no interrupts at all
        (None, InterruptMethod::Polling)
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(pcid_handle: &mut PcidServerHandle, address: usize) -> (Option<File>, InterruptMethod) {
    let pci_config = pcid_handle.fetch_config().expect("xhcid: failed to fetch config");

    if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        (Some(irq.irq_handle("xhcid")), InterruptMethod::Intx)
    } else {
        // no interrupts at all
        (None, InterruptMethod::Polling)
    }
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("xhcid: failed to daemonize");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PcidServerHandle::connect_default().expect("xhcid: failed to setup channel to pcid");
    let pci_config = pcid_handle.fetch_config().expect("xhcid: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_xhci");

    let _logger_ref = setup_logging(&name);

    log::debug!("XHCI PCI CONFIG: {:?}", pci_config);
    let bar = &pci_config.func.bars[0];

    let address = unsafe { bar.physmap_mem("xhcid") } as usize;

    let (irq_file, interrupt_method) = get_int_method(&mut pcid_handle, address);

    println!(" + XHCI {}", pci_config.func.display());

    let scheme_name = format!("usb.{}", name);
    let socket_fd = libredox::call::open(
        format!(":{}", scheme_name),
        flag::O_RDWR | flag::O_CREAT,
        0,
    )
    .expect("xhcid: failed to create usb scheme");
    let socket = Arc::new(Mutex::new(unsafe {
        File::from_raw_fd(socket_fd as RawFd)
    }));

    daemon.ready().expect("xhcid: failed to notify parent");

    let hci = Arc::new(Xhci::new(scheme_name, address, interrupt_method, pcid_handle).expect("xhcid: failed to allocate device"));
    xhci::start_irq_reactor(&hci, irq_file);
    futures::executor::block_on(hci.probe()).expect("xhcid: failed to probe");

    //let event_queue = RawEventQueue::new().expect("xhcid: failed to create event queue");

    libredox::call::setrens(0, 0).expect("xhcid: failed to enter null namespace");

    let todo = Arc::new(Mutex::new(Vec::<Packet>::new()));
    //let todo_futures = Arc::new(Mutex::new(Vec::<Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>>::new()));

    //let socket_fd = socket.lock().unwrap().as_raw_fd();
    //event_queue.subscribe(socket_fd as usize, 0, event::EventFlags::READ).unwrap();

    let socket_packet = socket.clone();

    loop {
        let mut socket = socket_packet.lock().unwrap();
        let mut todo = todo.lock().unwrap();

        let mut packet = Packet::default();
        match socket.read(&mut packet) {
            Ok(0) => break,
            Ok(_) => (),
            Err(err) => panic!("xhcid failed to read from socket: {err}"),
        }

        let a = packet.a;
        hci.handle(&mut packet);
        if packet.a == (-EWOULDBLOCK) as usize {
            packet.a = a;
            todo.push(packet);
        } else {
            socket.write(&packet).expect("xhcid failed to write to socket");
        }
    }

    std::process::exit(0);
}
