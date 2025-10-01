//! The Universal Serial Bus (USB) Module
//!
//! The implementations in this module are common to all USB interfaces (though individual elements
//! may be specific to only 2.0 or 3.2), and are used by specialized driver components like [xhci]
//! to implement the driver interface.
//!
//! The [Universal Serial Bus Specification](https://www.usb.org/document-library/usb-20-specification) and the [Universal Serial Bus 3.2 Specification](https://usb.org/document-library/usb-32-revision-11-june-2022) are
//! the documents that inform this implementation.
//!
//! See the crate-level documentation for the acronyms used to refer to specific documents.
pub use self::bos::{bos_capability_descs, BosAnyDevDesc, BosDescriptor, BosSuperSpeedDesc};
pub use self::config::ConfigDescriptor;
pub use self::device::{DeviceDescriptor, DeviceDescriptor8Byte};
pub use self::endpoint::{
    EndpointDescriptor, EndpointTy, HidDescriptor, SuperSpeedCompanionDescriptor,
    SuperSpeedPlusIsochCmpDescriptor, ENDP_ATTR_TY_MASK,
};
pub use self::hub::*;
pub use self::interface::InterfaceDescriptor;
pub use self::setup::{Setup, SetupReq};

/// Enumerates the list of descriptor kinds that can be reported by a USB device to report its
/// attributes to the system. (See USB32 Sections 9.5 and 9.6)
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum DescriptorKind {
    /// No Descriptor TODO: Determine why this state exists, and what it does in the code.
    None = 0,
    /// A Device Descriptor. See [DeviceDescriptor]
    Device = 1,
    /// A Configuration Descriptor. See [ConfigDescriptor]
    Configuration = 2,
    /// A String Descriptor. See (USB32 Section 9.6.9).
    String = 3,
    /// An Interface Descriptor. See [InterfaceDescriptor]
    Interface = 4,
    /// An Endpoint Descriptor. See [EndpointDescriptor]
    Endpoint = 5,
    /// A Device Qualifier. USB2-specific. See [DeviceQualifier]
    DeviceQualifier = 6,
    /// The "Other Speed Configuration" descriptor. USB2-specific. See (USB2 9.6.4]
    OtherSpeedConfiguration = 7,
    /// TODO: Determine the standard that specifies this
    InterfacePower = 8,
    /// TODO: Determine the standard that specifies this (Possibly USB-C?)
    OnTheGo = 9,
    /// A Binary Device Object Store Descriptor. See [BosDescriptor]
    BinaryObjectStorage = 15,
    /// TODO: Track down the HID standard for references
    Hid = 33,
    /// A USB Hub Device Descriptor. See [HubDescriptor]
    Hub = 41,
    /// A Super Speed Endpoint Companion Descriptor. See [SuperSpeedCompanionDescriptor]
    SuperSpeedCompanion = 48,
}

pub(crate) mod bos;
pub(crate) mod config;
pub(crate) mod device;
pub(crate) mod endpoint;
pub(crate) mod hub;
pub(crate) mod interface;
pub(crate) mod setup;
