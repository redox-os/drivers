//! [6 Reserved Feature Bits](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-6600006)
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

/// Negotiating this feature indicates that the driver can use descriptors
/// with the VIRTQ_DESC_F_INDIRECT flag set as described in 2.7.5.3 Indirect
/// Descriptors and 2.8.7 Indirect Flag: Scatter-Gather Support.
pub const VIRTIO_F_INDIRECT_DESC: u32 = 28;

/// This feature enables the used_event and the avail_event fields as
/// described in 2.7.7, 2.7.8 and 2.8.10.
pub const VIRTIO_F_EVENT_IDX: u32 = 29;

/// This indicates compliance with this specification, giving a simple way
/// to detect legacy devices or drivers.
pub const VIRTIO_F_VERSION_1: u32 = 32;

/// This feature indicates that the device can be used on a platform where device
/// access to data in memory is limited and/or translated. E.g. this is the case
/// if the device can be located behind an IOMMU that translates bus addresses
/// from the device into physical addresses in memory, if the device can be limited
/// to only access certain memory addresses or if special commands such as a cache
/// flush can be needed to synchronise data in memory with the device. Whether
/// accesses are actually limited or translated is described by platform-specific
/// means. If this feature bit is set to 0, then the device has same access to
/// memory addresses supplied to it as the driver has. In particular, the device
/// will always use physical addresses matching addresses used by the driver
/// (typically meaning physical addresses used by the CPU) and not translated
/// further, and can access any address supplied to it by the driver. When clear,
/// this overrides any platform-specific description of whether device access is
/// limited or translated in any way, e.g. whether an IOMMU may be present.
pub const VIRTIO_F_ACCESS_PLATFORM: u32 = 33;

/// This feature indicates support for the packed virtqueue layout as described
/// in 2.8 Packed Virtqueues.
pub const VIRTIO_F_RING_PACKED: u32 = 34;

/// This feature indicates that all buffers are used by the device in the same order
/// in which they have been made available.
pub const VIRTIO_F_IN_ORDER: u32 = 35;

/// This feature indicates that memory accesses by the driver and the device are
/// ordered in a way described by the platform.
/// If this feature bit is negotiated, the ordering in effect for any memory
/// accesses by the driver that need to be ordered in a specific way with respect
/// to accesses by the device is the one suitable for devices described by the
/// platform. This implies that the driver needs to use memory barriers suitable
/// for devices described by the platform; e.g. for the PCI transport in the case
/// of hardware PCI devices.
///
/// If this feature bit is not negotiated, then the device and driver are assumed
/// to be implemented in software, that is they can be assumed to run on identical
/// CPUs in an SMP configuration. Thus a weaker form of memory barriers is sufficient
/// to yield better performance.
pub const VIRTIO_F_ORDER_PLATFORM: u32 = 36;

/// This feature indicates that the device supports Single Root I/O Virtualization.
/// Currently only PCI devices support this feature.
pub const VIRTIO_F_SR_IOV: u32 = 37;

/// This feature indicates that the driver passes extra data (besides identifying
/// the virtqueue) in its device notifications. See 2.9 Driver Notifications.
pub const VIRTIO_F_NOTIFICATION_DATA: u32 = 38;

/// This feature indicates that the driver uses the data provided by the device as
/// a virtqueue identifier in available buffer notifications. As mentioned in section
/// 2.9, when the driver is required to send an available buffer notification to the
/// device, it sends the virtqueue number to be notified. The method of delivering
/// notifications is transport specific. With the PCI transport, the device can
/// optionally provide a per-virtqueue value for the driver to use in driver
/// notifications, instead of the virtqueue number. Some devices may benefit from this
/// flexibility by providing, for example, an internal virtqueue identifier, or an
/// internal offset related to the virtqueue number.
///
/// This feature indicates the availability of such value. The definition of the data
/// to be provided in driver notification and the delivery method is transport
/// specific. For more details about driver notifications over PCI see 4.1.5.2.
pub const VIRTIO_F_NOTIF_CONFIG_DATA: u32 = 39;

/// This feature indicates that the driver can reset a queue individually. See 2.6.1.
pub const VIRTIO_F_RING_RESET: u32 = 40;
