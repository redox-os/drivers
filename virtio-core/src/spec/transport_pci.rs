//! [4.1 Virtio Over PCI Bus](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-1150001)
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

use super::DeviceStatusFlags;
use crate::utils::VolatileCell;
use static_assertions::const_assert_eq;

/// [4.1.4 Virtio Structure PCI Capabilities](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-1240004)
#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct PciCapability {
    /// Identifies the structure.
    pub cfg_type: CfgType,
    /// Where to find it.
    pub bar: u8,
    /// Multiple capabilities of the same type.
    pub id: u8,
    /// Pad to a full dword.
    pub padding: [u8; 2],
    /// Offset within the bar.
    pub offset: u32,
    /// Length of the structure, in bytes.
    pub length: u32,
}

// The size of `PciCapability` is 13 bytes since the generic
// PCI fields are *not* included.
const_assert_eq!(core::mem::size_of::<PciCapability>(), 13);

#[derive(Debug, Copy, Clone)]
#[repr(u8)]
pub enum CfgType {
    /// Common Configuration.
    Common = 1,
    /// Notifications.
    Notify = 2,
    /// ISR Status.
    Isr = 3,
    /// Device specific configuration.
    Device = 4,
    /// PCI configuration access.
    PciConfig = 5,
    /// Shared memory region.
    SharedMemory = 8,
    /// Vendor-specific data.
    Vendor = 9,
}

const_assert_eq!(core::mem::size_of::<CfgType>(), 1);

#[derive(Debug)]
#[repr(C)]
pub struct CommonCfg {
    // About the whole device.
    /// The driver uses this to select which feature bits device_feature shows.
    /// Value 0x0 selects Feature Bits 0 to 31, 0x1 selects Feature Bits 32 to 63, etc.
    /// read-write
    pub device_feature_select: VolatileCell<u32>,
    /// The device uses this to report which feature bits it is offering to the driver:
    /// the driver writes to device_feature_select to select which feature bits are presented.
    /// read-only for driver
    pub device_feature: VolatileCell<u32>,
    /// The driver uses this to select which feature bits driver_feature shows.
    /// Value 0x0 selects Feature Bits 0 to 31, 0x1 selects Feature Bits 32 to 63, etc.
    /// read-write
    pub driver_feature_select: VolatileCell<u32>,
    /// The driver writes this to accept feature bits offered by the device.
    /// Driver Feature Bits selected by driver_feature_select.
    /// read-write
    pub driver_feature: VolatileCell<u32>,
    /// The driver sets the Configuration Vector for MSI-X.
    /// read-write
    pub config_msix_vector: VolatileCell<u16>,
    /// The device specifies the maximum number of virtqueues supported here.
    /// read-only for driver
    pub num_queues: VolatileCell<u16>,
    /// The driver writes the device status here (see 2.1).
    /// Writing 0 into this field resets the device.
    /// read-write
    pub device_status: VolatileCell<DeviceStatusFlags>,
    /// Configuration atomicity value. The device changes this every time the
    /// configuration noticeably changes.
    /// read-only for driver
    pub config_generation: VolatileCell<u8>,

    // About a specific virtqueue.
    /// Queue Select. The driver selects which virtqueue the following fields refer to.
    /// read-write
    pub queue_select: VolatileCell<u16>,
    /// Queue Size. On reset, specifies the maximum queue size supported by the device.
    /// This can be modified by the driver to reduce memory requirements.
    /// A 0 means the queue is unavailable.
    /// read-write
    pub queue_size: VolatileCell<u16>,
    /// The driver uses this to specify the queue vector for MSI-X.
    /// read-write
    pub queue_msix_vector: VolatileCell<u16>,
    /// The driver uses this to selectively prevent the device from executing
    /// requests from this virtqueue. 1 - enabled; 0 - disabled.
    /// read-write
    pub queue_enable: VolatileCell<u16>,
    /// The driver reads this to calculate the offset from start of Notification
    /// structure at which this virtqueue is located. Note: this is not an offset
    /// in bytes. See 4.1.4.4 below.
    /// read-only for driver
    pub queue_notify_off: VolatileCell<u16>,
    /// The driver writes the physical address of Descriptor Area here.
    /// See section 2.6.
    /// read-write
    pub queue_desc: VolatileCell<u64>,
    /// The driver writes the physical address of Driver Area here.
    /// See section 2.6.
    /// read-write
    pub queue_driver: VolatileCell<u64>,
    /// The driver writes the physical address of Device Area here.
    /// See section 2.6.
    /// read-write
    pub queue_device: VolatileCell<u64>,
    /// This field exists only if VIRTIO_F_NOTIF_CONFIG_DATA has been negotiated.
    /// The driver will use this value to put it in the ’virtqueue number’ field
    /// in the available buffer notification structure. See section 4.1.5.2. Note:
    /// This field provides the device with flexibility to determine how virtqueues
    /// will be referred to in available buffer notifications. In a trivial case the
    /// device can set queue_notify_data=vqn. Some devices may benefit from providing
    /// another value, for example an internal virtqueue identifier, or an internal
    /// offset related to the virtqueue number.
    /// read-only for driver
    pub queue_notify_data: VolatileCell<u16>,
    /// The driver uses this to selectively reset the queue. This field exists
    /// only if VIRTIO_F_RING_RESET has been negotiated. (see 2.6.1).
    /// read-write
    pub queue_reset: VolatileCell<u16>,
}

//TODO: why does this fail on x86?
#[cfg(not(target_arch = "x86"))]
const_assert_eq!(core::mem::size_of::<CommonCfg>(), 64);

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct PciCapabilityNotify {
    pub cap: PciCapability,
    /// Multiplier for queue_notify_off.
    notify_off_multiplier: u32,
}

impl PciCapabilityNotify {
    pub fn notify_off_multiplier(&self) -> u32 {
        self.notify_off_multiplier
    }
}

const_assert_eq!(core::mem::size_of::<PciCapabilityNotify>(), 17);

/// Vector value used to disable MSI for queue
pub const VIRTIO_MSI_NO_VECTOR: u16 = 0xffff;
