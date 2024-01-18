//! https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html
//!
//! This file contains comments copied from the VirtIO specification which are
//! licensed under the following conditions:
//!
//! Copyright © OASIS Open 2022. All Rights Reserved.
//!
//! All capitalized terms in the following text have the meanings assigned to them
//! in the OASIS Intellectual Property Rights Policy (the "OASIS IPR Policy"). The
//! full Policy may be found at the OASIS website.
//!
//! This document and translations of it may be copied and furnished to others,
//! and derivative works that comment on or otherwise explain it or assist in its
//! implementation may be prepared, copied, published, and distributed, in whole
//! or in part, without restriction of any kind, provided that the above copyright
//! notice and this section are included on all such copies and derivative works.
//! However, this document itself may not be modified in any way, including by
//! removing the copyright notice or references to OASIS, except as needed for the
//! purpose of developing any document or deliverable produced by an OASIS Technical
//! Committee (in which case the rules applicable to copyrights, as set forth in the
//! OASIS IPR Policy, must be followed) or as required to translate it into languages
//! other than English.

bitflags::bitflags! {
    /// [2.1 Device Status Field](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-110001)
    #[derive(Debug, Copy, Clone, PartialEq)]
    #[repr(transparent)]
    pub struct DeviceStatusFlags: u8 {
        /// Indicates that the guest OS has found the device and recognized it as a
        /// valid device.
        const ACKNOWLEDGE = 1;
        /// Indicates that the guest OS knows how to drive the device.
        const DRIVER = 2;
        /// Indicates that something went wrong in the guest and it has given up on
        /// the device.
        const FAILED = 128;
        /// Indicates that the driver has acknowledged all the features it understands
        /// and feature negotiation is complete.
        const FEATURES_OK = 8;
        /// Indicates that the driver is set up and ready to drive the device.
        const DRIVER_OK = 4;
        /// Indicates that the device has experienced an error from which it can’t recover.
        const DEVICE_NEEDS_RESET = 64;
    }
}

mod split_virtqueue;
pub use split_virtqueue::*;

// FIXME add [2.8 Packed Virtqueues](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-720008)

mod transport_pci;
pub use transport_pci::*;

mod reserved_features;
pub use reserved_features::*;
