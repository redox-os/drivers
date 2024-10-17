//! The eXtensible Host Controller Interface (XHCI) Daemon
//!
//! This crate provides the executable xhcid daemon that implements the driver for interacting with
//! a PCIe XHCI device
//!
//! XHCI is a standard for the USB Host Controller interface specified by Intel that provides a
//! common register interface for systems to use to interact with the Universal Serial Bus (USB)
//! subsystem.
//!
//! USB consists of three types of devices: The Host Controller/Root Hub, USB Hubs, and Endpoints.
//! Endpoints represent actual devices connected to the USB fabric. USB Hubs are intermediaries
//! between the Host Controller and the endpoints that report when devices have been connected/disconnected.
//! The Host Controller provides the interface to the USB subsystem that software running on the
//! system's CPU can interact with. It's a tree-like structure, which the Host Controller enumerating
//! and addressing all the hubs and endpoints in the tree. Data then flows through the fabric
//! using the USB protocol (2.0 or 3.2) as packets. Hubs have multiple ports that endpoints can
//! connect to, and they notify the Host Controller/Root Hub when devices are hot plugged or removed.
//!
//! This documentation will refer directly to the relevant standards, which are as follows:
//!
//! - XHCI  - [eXtensible Host Controller Interface for Universal Serial Bus (xHCI) Requirements Specification](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf)
//! - USB2  - [Universal Serial Bus Specification](https://www.usb.org/document-library/usb-20-specification)
//! - USB32 - [Universal Serial Bus 3.2 Specification Revision 1.1](https://usb.org/document-library/usb-32-revision-11-june-2022)
//!
#![warn(missing_docs)]
#[macro_use]
extern crate bitflags;

use std::convert::{TryFrom, TryInto};
use std::env;
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use libredox::flag;
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::allocate_single_interrupt_vector_for_msi;
use pcid_interface::irq_helpers::read_bsp_apic_id;
use pcid_interface::msi::MsixTableEntry;
use pcid_interface::{
    MsiSetFeatureInfo, PciFeature, PciFeatureInfo, PciFunctionHandle, SetFeatureInfo,
};

use common::io::Io;
use event::{Event, RawEventQueue};
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::flag::EventFlags;
use syscall::scheme::Scheme;

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

#[cfg(target_arch = "x86_64")]
fn get_int_method(
    pcid_handle: &mut PciFunctionHandle,
    bar0_address: usize,
) -> (Option<File>, InterruptMethod) {
    let pci_config = pcid_handle.config();

    let all_pci_features = pcid_handle
        .fetch_all_features()
        .expect("xhcid: failed to fetch pci features");
    log::debug!("XHCI PCI FEATURES: {:?}", all_pci_features);

    let has_msi = all_pci_features.iter().any(|feature| feature.is_msi());
    let has_msix = all_pci_features.iter().any(|feature| feature.is_msix());

    if has_msi && !has_msix {
        let mut capability = match pcid_handle
            .feature_info(PciFeature::Msi)
            .expect("xhcid: failed to retrieve the MSI capability structure from pcid")
        {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
        let (msg_addr_and_data, interrupt_handle) =
            allocate_single_interrupt_vector_for_msi(destination_id);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address_and_data: Some(msg_addr_and_data),
            mask_bits: None,
        };
        pcid_handle
            .set_feature_info(SetFeatureInfo::Msi(set_feature_info))
            .expect("xhcid: failed to set feature info");

        pcid_handle
            .enable_feature(PciFeature::Msi)
            .expect("xhcid: failed to enable MSI");
        log::debug!("Enabled MSI");

        (Some(interrupt_handle), InterruptMethod::Msi)
    } else if has_msix {
        let msix_info = match pcid_handle
            .feature_info(PciFeature::MsiX)
            .expect("xhcid: failed to retrieve the MSI-X capability structure from pcid")
        {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        msix_info.validate(pci_config.func.bars);

        assert_eq!(msix_info.table_bar, 0);
        let virt_table_base =
            (bar0_address + msix_info.table_offset as usize) as *mut MsixTableEntry;

        let mut info = xhci::MappedMsixRegs {
            virt_table_base: NonNull::new(virt_table_base).unwrap(),
            info: msix_info,
        };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) =
                allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            (
                Some(interrupt_handle),
                InterruptMethod::MsiX(Mutex::new(info)),
            )
        };

        pcid_handle
            .enable_feature(PciFeature::MsiX)
            .expect("xhcid: failed to enable MSI-X");
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
fn get_int_method(
    pcid_handle: &mut PciFunctionHandle,
    address: usize,
) -> (Option<File>, InterruptMethod) {
    let pci_config = pcid_handle.config();

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
    let mut pcid_handle =
        PciFunctionHandle::connect_default().expect("xhcid: failed to setup channel to pcid");
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_xhci");

    common::setup_logging(
        "usb",
        "host",
        &name,
        log::LevelFilter::Info,
        log::LevelFilter::Debug,
    );

    log::debug!("XHCI PCI CONFIG: {:?}", pci_config);

    let address = unsafe { pcid_handle.map_bar(0) }
        .expect("xhcid")
        .ptr
        .as_ptr() as usize;

    let (irq_file, interrupt_method) = (None, InterruptMethod::Polling);
    // TODO: fix interrutps: get_int_method(&mut pcid_handle, address);

    println!(" + XHCI {}", pci_config.func.display());

    let scheme_name = format!("usb.{}", name);
    let socket_fd =
        libredox::call::open(format!(":{}", scheme_name), flag::O_RDWR | flag::O_CREAT, 0)
            .expect("xhcid: failed to create usb scheme");
    let socket = Arc::new(Mutex::new(unsafe { File::from_raw_fd(socket_fd as RawFd) }));

    daemon.ready().expect("xhcid: failed to notify parent");

    let hci = Arc::new(
        Xhci::new(scheme_name, address, interrupt_method, pcid_handle)
            .expect("xhcid: failed to allocate device"),
    );
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
            socket
                .write(&packet)
                .expect("xhcid failed to write to socket");
        }
    }

    std::process::exit(0);
}
