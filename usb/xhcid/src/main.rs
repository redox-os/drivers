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
#![allow(warnings)]

#[macro_use]
extern crate bitflags;

use std::fs::File;
use std::sync::Arc;

use pcid_interface::irq_helpers::read_bsp_apic_id;
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::{
    allocate_first_msi_interrupt_on_bsp, allocate_single_interrupt_vector_for_msi,
};
use pcid_interface::{PciFeature, PciFeatureInfo, PciFunctionHandle};

use redox_scheme::{RequestKind, SignalBehavior, Socket};

use crate::xhci::{InterruptMethod, Xhci};

// Declare as pub so that no warnings appear due to parts of the interface code not being used by
// the driver. Since there's also a dedicated crate for the driver interface, those warnings don't
// mean anything.
pub mod driver_interface;

mod usb;
mod xhci;

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> (Option<File>, InterruptMethod) {
    let pci_config = pcid_handle.config();

    let all_pci_features = pcid_handle.fetch_all_features();
    log::debug!("XHCI PCI FEATURES: {:?}", all_pci_features);

    let has_msi = all_pci_features.iter().any(|feature| feature.is_msi());
    let has_msix = all_pci_features.iter().any(|feature| feature.is_msix());

    if has_msix {
        let msix_info = match pcid_handle.feature_info(PciFeature::MsiX) {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        let mut info = unsafe { msix_info.map_and_mask_all(pcid_handle) };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) =
                allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            (Some(interrupt_handle), InterruptMethod::Msi)
        };

        pcid_handle.enable_feature(PciFeature::MsiX);
        log::debug!("Enabled MSI-X");

        method
    } else if has_msi {
        let interrupt_handle = allocate_first_msi_interrupt_on_bsp(pcid_handle);
        (Some(interrupt_handle), InterruptMethod::Msi)
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
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> (Option<File>, InterruptMethod) {
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

//TODO: cleanup CSZ support
fn daemon_with_context_size<const N: usize>(
    daemon: redox_daemon::Daemon,
    mut pcid_handle: PciFunctionHandle,
) -> ! {
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_xhci");

    common::setup_logging(
        "usb",
        "host",
        &name,
        log::LevelFilter::Warn,
        common::file_level(),
    );

    log::debug!("XHCI PCI CONFIG: {:?}", pci_config);

    let address = unsafe { pcid_handle.map_bar(0) }.ptr.as_ptr() as usize;

    let (irq_file, interrupt_method) = (None, InterruptMethod::Polling); //get_int_method(&mut pcid_handle);
                                                                         //TODO: Fix interrupts.

    log::info!("XHCI {}", pci_config.func.display());

    let scheme_name = format!("usb.{}", name);
    let socket = Socket::create(scheme_name.clone()).expect("xhcid: failed to create usb scheme");

    daemon.ready().expect("xhcid: failed to notify parent");

    let hci = Arc::new(
        Xhci::<N>::new(scheme_name, address, interrupt_method, pcid_handle)
            .expect("xhcid: failed to allocate device"),
    );

    xhci::start_irq_reactor(&hci, irq_file);
    xhci::start_device_enumerator(&hci);

    hci.poll();

    loop {
        let Some(request) = socket
            .next_request(SignalBehavior::Restart)
            .expect("xhcid: failed to read scheme")
        else {
            // Scheme likely got unmounted
            std::process::exit(0);
        };

        match request.kind() {
            RequestKind::Call(call_request) => {
                let resp = call_request.handle_sync(&mut &*hci);
                socket
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("xhcid: failed to write scheme");
            }
            RequestKind::OnClose { id } => {
                hci.on_close(id);
            }
            _ => {}
        }
    }
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let address = unsafe { pcid_handle.map_bar(0) }.ptr.as_ptr() as usize;
    let cap = unsafe { &mut *(address as *mut xhci::CapabilityRegs) };
    if cap.csz() {
        daemon_with_context_size::<{ xhci::CONTEXT_64 }>(daemon, pcid_handle)
    } else {
        daemon_with_context_size::<{ xhci::CONTEXT_32 }>(daemon, pcid_handle)
    }
}
