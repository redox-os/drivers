//! https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html

use core::cell::UnsafeCell;
use static_assertions::const_assert_eq;

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

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct PciCapability {
    /// Identifies the structure.
    pub cfg_type: CfgType,
    /// Where to find it.
    pub bar: u8,
    /// Pad to a full dword.
    pub padding: [u8; 3],
    /// Offset within the bar.
    pub offset: u32,
    /// Length of the structure, in bytes.
    pub length: u32,
}

// The size of `PciCapability` is 13 bytes since
// the generic PCI fields are *not* included.
const_assert_eq!(core::mem::size_of::<PciCapability>(), 13);

bitflags::bitflags! {
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

#[derive(Debug)]
#[repr(C)]
pub struct CommonCfg {
    // About the whole device.
    pub device_feature_select: VolatileCell<u32>, // read-write
    pub device_feature: VolatileCell<u32>,        // read-only for driver
    pub driver_feature_select: VolatileCell<u32>, // read-write
    pub driver_feature: VolatileCell<u32>,        // read-write
    pub msix_config: VolatileCell<u16>,           // read-write
    pub num_queues: VolatileCell<u16>,            // read-only for driver
    pub device_status: VolatileCell<DeviceStatusFlags>, // read-write
    pub config_generation: VolatileCell<u8>,      // read-only for driver

    // About a specific virtqueue.
    pub queue_select: VolatileCell<u16>,      // read-write
    pub queue_size: VolatileCell<u16>,        // read-write
    pub queue_msix_vector: VolatileCell<u16>, // read-write
    pub queue_enable: VolatileCell<u16>,      // read-write
    pub queue_notify_off: VolatileCell<u16>,  // read-only for driver
    pub queue_desc: VolatileCell<u64>,        // read-write
    pub queue_driver: VolatileCell<u64>,      // read-write
    pub queue_device: VolatileCell<u64>,      // read-write
}

const_assert_eq!(core::mem::size_of::<CommonCfg>(), 56);

bitflags::bitflags! {
    #[repr(transparent)]
    pub struct DescriptorFlags: u16 {
        /// The next field contains linked buffer index.
        const NEXT       = 1 << 0;
        /// The buffer is write-only (otherwise read-only).
        const WRITE_ONLY = 1 << 1;
        /// The buffer contains a list of buffer descriptors.
        const INDIRECT   = 1 << 2;
    }
}

#[repr(C)]
pub struct Descriptor {
    /// Address (guest-physical).
    address: u64,
    /// Size of the descriptor.
    size: u32,
    flags: DescriptorFlags,
    /// Index of next desciptor in chain.
    next: u16,
}

const_assert_eq!(core::mem::size_of::<Descriptor>(), 16);

pub struct VirtQueue {}

pub struct StandardTransport {}

impl StandardTransport {
    pub fn new() -> Self {
        Self {}
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct VolatileCell<T> {
    value: UnsafeCell<T>,
}

impl<T: Copy> VolatileCell<T> {
    /// Returns a copy of the contained value.
    #[inline]
    pub fn get(&self) -> T {
        unsafe { core::ptr::read_volatile(self.value.get()) }
    }

    /// Sets the contained value.
    #[inline]
    pub fn set(&self, value: T) {
        unsafe { core::ptr::write_volatile(self.value.get(), value) }
    }
}
