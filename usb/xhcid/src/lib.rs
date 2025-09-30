//! The eXtensible Host Controller Interface (XHCI) Daemon Interface
//!
//! This crate implements the driver interface for interacting with the Redox xhcid daemon from
//! another userspace process.
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
pub extern crate plain;

mod driver_interface;
pub mod usb;

pub use driver_interface::*;
