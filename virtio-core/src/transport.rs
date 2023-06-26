use crate::spec::*;
use crate::utils::{align, VolatileCell};

use syscall::{Dma, PHYSMAP_WRITE};

use core::mem::size_of;
use core::sync::atomic::{fence, AtomicU16, Ordering};

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("syscall failed")]
    SyscallError(syscall::Error),
    #[error("pcid client handle error")]
    PcidClientHandle(pcid_interface::PcidClientHandleError),
    #[error("the device is incapable of {0:?}")]
    InCapable(CfgType),
}

impl From<pcid_interface::PcidClientHandleError> for Error {
    fn from(value: pcid_interface::PcidClientHandleError) -> Self {
        Self::PcidClientHandle(value)
    }
}

impl From<syscall::Error> for Error {
    fn from(value: syscall::Error) -> Self {
        Self::SyscallError(value)
    }
}

/// Returns the queue part sizes in bytes.
///
/// ## Reference
/// Section 2.7 Split Virtqueues of the specfication v1.2 describes the alignment
/// and size of the queue parts.
///
/// ## Panics
/// If `queue_size` is not a power of two or is zero.
const fn queue_part_sizes(queue_size: usize) -> (usize, usize, usize) {
    assert!(queue_size.is_power_of_two() && queue_size != 0);

    const DESCRIPTOR_ALIGN: usize = 16;
    const AVAILABLE_ALIGN: usize = 2;
    const USED_ALIGN: usize = 4;

    let queue_size = queue_size as usize;
    let desc = size_of::<Descriptor>() * queue_size;

    // `avail_header`: Size of the available ring header and the footer.
    let avail_header = size_of::<AvailableRing>() + size_of::<AvailableRingExtra>();
    let avail = avail_header + size_of::<AvailableRingElement>() * queue_size;

    // `used_header`: Size of the used ring header and the footer.
    let used_header = size_of::<UsedRing>() + size_of::<UsedRingExtra>();
    let used = used_header + size_of::<UsedRingElement>() * queue_size;

    (
        align(desc, DESCRIPTOR_ALIGN),
        align(avail, AVAILABLE_ALIGN),
        align(used, USED_ALIGN),
    )
}

pub struct QueueInner<'a> {
    pub descriptor: Dma<[Descriptor]>,
    pub available: Available<'a>,
    pub used: Used<'a>,

    /// Keeps track of unused descriptor indicies.
    pub descriptor_stack: VecDeque<u16>,

    notification_bell: &'a mut VolatileCell<u16>,
    head_index: u16,
}

unsafe impl Sync for QueueInner<'_> {}
unsafe impl Send for QueueInner<'_> {}

pub struct Queue<'a> {
    pub inner: Mutex<QueueInner<'a>>,
    sref: Weak<Self>,
}

impl<'a> Queue<'a> {
    pub fn new(
        descriptor: Dma<[Descriptor]>,
        available: Available<'a>,
        used: Used<'a>,

        notification_bell: &'a mut VolatileCell<u16>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|sref| Self {
            inner: Mutex::new(QueueInner {
                head_index: 0,
                descriptor_stack: (0..(descriptor.len() - 1) as u16).rev().collect(),

                descriptor,
                available,
                used,

                notification_bell,
            }),

            sref: sref.clone(),
        })
    }

    pub fn send(&self, chain: Vec<Buffer>) {
        let mut first_descriptor: Option<usize> = None;
        let mut last_descriptor: Option<usize> = None;

        for buffer in chain.iter() {
            let descriptor = self.alloc_descriptor();

            let mut inner = self.inner.lock().unwrap();

            if first_descriptor.is_none() {
                first_descriptor = Some(descriptor);
            }

            inner.descriptor[descriptor].address = buffer.buffer as u64;
            inner.descriptor[descriptor].flags = buffer.flags;
            inner.descriptor[descriptor].size = buffer.size as u32;

            if let Some(index) = last_descriptor {
                inner.descriptor[index].next = descriptor as u16;
            }

            last_descriptor = Some(descriptor);
        }

        let mut inner = self.inner.lock().unwrap();

        let last_descriptor = last_descriptor.unwrap();
        let first_descriptor = first_descriptor.unwrap();

        inner.descriptor[last_descriptor].next = 0;

        fence(Ordering::SeqCst);
        let index = inner.head_index as usize;

        inner
            .available
            .get_element_at(index)
            .table_index
            .set(first_descriptor as u16);

        fence(Ordering::SeqCst);
        inner.available.set_head_idx(index as u16 + 1);
        inner.head_index += 1;

        assert_eq!(inner.used.flags(), 0);
        inner.notification_bell.set(0); // FIXME: This corresponds to the queue index.
    }

    fn alloc_descriptor(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();

        if let Some(index) = inner.descriptor_stack.pop_front() {
            index as usize
        } else {
            log::warn!("virtiod: descriptors exhausted, waiting on garabage collector");
            drop(inner);

            // Wait for the garbage collector thread to release some descriptors.
            //
            // TODO(andypython): Instead of just yielding, we should have a proper notificiation
            //                   mechanism. I am not aware whats the standard way redox applications
            //                   or drivers implement basically a WaitQueue which you can use to wake
            //                   up a thread. The descripts really should NEVER run out, but if they
            //                   do, have a proper way to handle them.
            std::thread::yield_now();
            self.alloc_descriptor()
        }
    }
}

pub struct Available<'a> {
    addr: usize,
    size: usize,

    queue_size: usize,

    ring: &'a mut AvailableRing,
}

impl<'a> Available<'a> {
    pub fn new(queue_size: usize) -> Result<Self, Error> {
        let (_, size, _) = queue_part_sizes(queue_size);
        let size = size.next_multiple_of(syscall::PAGE_SIZE); // align to page size

        let addr = unsafe { syscall::physalloc(size) }.map_err(Error::SyscallError)?;
        let virt =
            unsafe { syscall::physmap(addr, size, PHYSMAP_WRITE) }.map_err(Error::SyscallError)?;

        let ring = unsafe { &mut *(virt as *mut AvailableRing) };

        Ok(Self {
            addr,
            size,
            ring,
            queue_size,
        })
    }

    /// ## Panics
    /// This function panics if the index is out of bounds.
    pub fn get_element_at(&mut self, index: usize) -> &mut AvailableRingElement {
        // SAFETY: We have exclusive access to the elements and the number of elements
        //         is correct; same as the queue size.
        unsafe {
            self.ring
                .elements
                .as_mut_slice(self.queue_size)
                .get_mut(index % 256)
                .expect("virtio::available: index out of bounds")
        }
    }

    pub fn set_head_idx(&mut self, index: u16) {
        self.ring.head_index.set(index);
    }

    pub fn phys_addr(&self) -> usize {
        self.addr
    }
}

impl Drop for Available<'_> {
    fn drop(&mut self) {
        log::warn!("virtio: dropping 'available' ring at {:#x}", self.addr);

        unsafe {
            syscall::physunmap(self.addr).unwrap();
            syscall::physfree(self.addr, self.size).unwrap();
        }
    }
}

pub struct Used<'a> {
    addr: usize,
    size: usize,

    queue_size: usize,

    ring: &'a mut UsedRing,
}

impl<'a> Used<'a> {
    pub fn new(queue_size: usize) -> Result<Self, Error> {
        let (_, _, size) = queue_part_sizes(queue_size);
        let size = size.next_multiple_of(syscall::PAGE_SIZE); // align to page size

        let addr = unsafe { syscall::physalloc(size) }.map_err(Error::SyscallError)?;
        let virt =
            unsafe { syscall::physmap(addr, size, PHYSMAP_WRITE) }.map_err(Error::SyscallError)?;

        let ring = unsafe { &mut *(virt as *mut UsedRing) };

        Ok(Self {
            addr,
            size,
            ring,
            queue_size,
        })
    }

    /// ## Panics
    /// This function panics if the index is out of bounds.
    pub fn get_element_at(&mut self, index: usize) -> &mut UsedRingElement {
        // SAFETY: We have exclusive access to the elements and the number of elements
        //         is correct; same as the queue size.
        unsafe {
            self.ring
                .elements
                .as_mut_slice(self.queue_size)
                .get_mut(index % 256)
                .expect("virtio::used: index out of bounds")
        }
    }

    pub fn flags(&self) -> u16 {
        self.ring.flags.get()
    }

    pub fn head_index(&self) -> u16 {
        self.ring.head_index.get()
    }

    pub fn phys_addr(&self) -> usize {
        self.addr
    }
}

impl Drop for Used<'_> {
    fn drop(&mut self) {
        log::warn!("virtio: dropping 'used' ring at {:#x}", self.addr);

        unsafe {
            syscall::physunmap(self.addr).unwrap();
            syscall::physfree(self.addr, self.size).unwrap();
        }
    }
}

pub struct StandardTransport<'a> {
    common: Mutex<&'a mut CommonCfg>,
    notify: *const u8,
    notify_mul: u32,

    queue_index: AtomicU16,
    sref: Weak<Self>,
}

impl<'a> StandardTransport<'a> {
    pub fn new(common: &'a mut CommonCfg, notify: *const u8, notify_mul: u32) -> Arc<Self> {
        Arc::new_cyclic(|sref| Self {
            common: Mutex::new(common),
            notify,
            notify_mul,

            queue_index: AtomicU16::new(0),
            sref: sref.clone(),
        })
    }

    pub fn sref(&self) -> Arc<Self> {
        // UNWRAP: The constructor ensures that we are wrapped in our own `Arc`. So this
        //         unwrap is going to be unreachable.
        self.sref.upgrade().unwrap()
    }

    pub fn check_device_feature(&self, feature: u32) -> bool {
        let mut common = self.common.lock().unwrap();

        common.device_feature_select.set(feature >> 5);
        (common.device_feature.get() & (1 << (feature & 31))) != 0
    }

    pub fn ack_driver_feature(&self, feature: u32) {
        let mut common = self.common.lock().unwrap();

        common.driver_feature_select.set(feature >> 5);

        let current = common.driver_feature.get();
        common.driver_feature.set(current | (1 << (feature & 31)));
    }

    pub fn finalize_features(&self) {
        // Check VirtIO version 1 compliance.
        assert!(self.check_device_feature(VIRTIO_F_VERSION_1));
        self.ack_driver_feature(VIRTIO_F_VERSION_1);

        let mut common = self.common.lock().unwrap();

        let status = common.device_status.get();
        common
            .device_status
            .set(status | DeviceStatusFlags::FEATURES_OK);

        // Re-read device status to ensure the `FEATURES_OK` bit is still set: otherwise,
        // the device does not support our subset of features and the device is unusable.
        let confirm = common.device_status.get();
        assert!((confirm & DeviceStatusFlags::FEATURES_OK) == DeviceStatusFlags::FEATURES_OK);
    }

    pub fn run_device(&self) {
        let mut common = self.common.lock().unwrap();

        let status = common.device_status.get();
        common
            .device_status
            .set(status | DeviceStatusFlags::DRIVER_OK);
    }

    pub fn setup_queue(&self, vector: u16) -> Result<Arc<Queue<'a>>, Error> {
        let mut common = self.common.lock().unwrap();

        let queue_index = self.queue_index.fetch_add(1, Ordering::SeqCst);
        common.queue_select.set(queue_index);

        let queue_size = common.queue_size.get() as usize;
        let queue_notify_idx = common.queue_notify_off.get();

        log::info!("notify_idx: {}", queue_notify_idx);

        // Allocate memory for the queue structues.
        let descriptor = unsafe {
            Dma::<[Descriptor]>::zeroed_unsized(queue_size).map_err(Error::SyscallError)?
        };

        let mut avail = Available::new(queue_size)?;
        let mut used = Used::new(queue_size)?;

        for i in 0..queue_size {
            // XXX: Fill the `table_index` of the elements with `T::MAX` to help with
            //      debugging since qemu reports them as illegal values.
            avail.get_element_at(i).table_index.set(u16::MAX);
            used.get_element_at(i).table_index.set(u32::MAX);
        }

        common.queue_desc.set(descriptor.physical() as u64);
        common.queue_driver.set(avail.phys_addr() as u64);
        common.queue_device.set(used.phys_addr() as u64);

        // Set the MSI-X vector.
        common.queue_msix_vector.set(vector);
        assert!(common.queue_msix_vector.get() == vector);

        // Enable the queue.
        common.queue_enable.set(1);

        let notification_bell = unsafe {
            let offset = self.notify_mul * queue_notify_idx as u32;
            &mut *(self.notify.add(offset as usize) as *mut VolatileCell<u16>)
        };

        log::info!("virtio: enabled queue #{queue_index} (size={queue_size})");
        Ok(Queue::new(descriptor, avail, used, notification_bell))
    }
}
