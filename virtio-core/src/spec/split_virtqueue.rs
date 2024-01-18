//! [2.7 Split Virtqueues](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-350007)
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

use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};

use crate::utils::{IncompleteArrayField, VolatileCell};
use static_assertions::const_assert_eq;

/// [2.7.5 The Virtqueue Descriptor table](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-430005)
#[repr(C, align(16))]
pub struct Descriptor {
    /// Address (guest-physical).
    address: AtomicU64,
    /// Size of the descriptor.
    size: AtomicU32,
    flags: AtomicU16,
    /// Next field if flags & NEXT
    next: AtomicU16,
}

const_assert_eq!(core::mem::size_of::<Descriptor>(), 16);

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    #[repr(transparent)]
    pub struct DescriptorFlags: u16 {
        /// This marks a buffer as continuing via the next field.
        const NEXT = 1 << 0;
        /// This marks a buffer as device write-only (otherwise device read-only).
        const WRITE_ONLY = 1 << 1;
        /// This means the buffer contains a list of buffer descriptors.
        const INDIRECT = 1 << 2;
    }
}

impl Descriptor {
    pub fn set_addr(&self, addr: u64) {
        self.address.store(addr, Ordering::SeqCst)
    }

    pub fn set_size(&self, size: u32) {
        self.size.store(size, Ordering::SeqCst)
    }

    pub fn set_next(&self, next: Option<u16>) {
        self.next.store(next.unwrap_or_default(), Ordering::SeqCst)
    }

    pub fn set_flags(&self, flags: DescriptorFlags) {
        self.flags.store(flags.bits(), Ordering::SeqCst)
    }

    pub fn next(&self) -> u16 {
        self.next.load(Ordering::SeqCst)
    }

    pub fn flags(&self) -> DescriptorFlags {
        DescriptorFlags::from_bits_truncate(self.flags.load(Ordering::SeqCst))
    }
}

// ======== Available Ring ========
//
// XXX: The driver uses the available ring to offer buffers to the
//      device. Each ring entry refers to the head of a descriptor
//      chain.

/// [2.7.6 The Virtqueue Available Ring](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-490006)
#[repr(C, align(2))]
pub struct AvailableRing {
    pub flags: VolatileCell<u16>,
    pub head_index: AtomicU16,
    pub elements: IncompleteArrayField<AvailableRingElement>,
}

const_assert_eq!(core::mem::size_of::<AvailableRing>(), 4);

#[repr(C)]
pub struct AvailableRingElement {
    pub table_index: AtomicU16,
}

impl AvailableRingElement {
    pub fn set_table_index(&self, index: u16) {
        self.table_index.store(index, Ordering::SeqCst)
    }
}

const_assert_eq!(core::mem::size_of::<AvailableRingElement>(), 2);

#[repr(C)]
pub struct AvailableRingExtra {
    pub avail_event: VolatileCell<u16>, // Only if `VIRTIO_F_EVENT_IDX`
}

const_assert_eq!(core::mem::size_of::<AvailableRingExtra>(), 2);

// ======== Used Ring ========

/// [2.7.8 The Virtqueue Used Ring](https://docs.oasis-open.org/virtio/virtio/v1.2/cs01/virtio-v1.2-cs01.html#x1-540008)
#[repr(C, align(4))]
pub struct UsedRing {
    pub flags: VolatileCell<u16>,
    pub head_index: VolatileCell<u16>,
    pub elements: IncompleteArrayField<UsedRingElement>,
}

const_assert_eq!(core::mem::size_of::<UsedRing>(), 4);

#[repr(C)]
pub struct UsedRingElement {
    pub table_index: VolatileCell<u32>,
    pub written: VolatileCell<u32>,
}

const_assert_eq!(core::mem::size_of::<UsedRingElement>(), 8);

#[repr(C)]
pub struct UsedRingExtra {
    pub event_index: VolatileCell<u16>,
}

// ======== Utils ========
pub struct Buffer {
    pub(crate) buffer: usize,
    pub(crate) size: usize,
    pub(crate) flags: DescriptorFlags,
}

impl Buffer {
    pub fn new<T>(val: &common::dma::Dma<T>) -> Self {
        Self {
            buffer: val.physical(),
            size: core::mem::size_of::<T>(),
            flags: DescriptorFlags::empty(),
        }
    }

    pub fn new_unsized<T>(val: &common::dma::Dma<[T]>) -> Self {
        Self {
            buffer: val.physical(),
            size: core::mem::size_of::<T>() * val.len(),
            flags: DescriptorFlags::empty(),
        }
    }

    pub fn new_sized<T>(val: &common::dma::Dma<[T]>, size: usize) -> Self {
        Self {
            buffer: val.physical(),
            size,
            flags: DescriptorFlags::empty(),
        }
    }

    pub fn flags(mut self, flags: DescriptorFlags) -> Self {
        self.flags = flags;
        self
    }
}

/// XXX: The [`DescriptorFlags::NEXT`] flag is set automatically.
pub struct ChainBuilder {
    buffers: Vec<Buffer>,
}

impl ChainBuilder {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
        }
    }

    pub fn chain(mut self, mut buffer: Buffer) -> Self {
        buffer.flags |= DescriptorFlags::NEXT;
        self.buffers.push(buffer);
        self
    }

    pub fn build(mut self) -> Vec<Buffer> {
        let last_buffer = self.buffers.last_mut().expect("virtio-core: empty chain");
        last_buffer.flags.remove(DescriptorFlags::NEXT);

        self.buffers
    }
}
