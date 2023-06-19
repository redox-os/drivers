//! https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html

use static_assertions::const_assert_eq;

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
struct Descriptor {
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

pub struct Transport {}


