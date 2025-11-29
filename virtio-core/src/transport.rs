use crate::spec::*;
use crate::utils::align;

use common::dma::Dma;
use event::RawEventQueue;

use core::mem::size_of;
use core::sync::atomic::{AtomicU16, Ordering};

use std::fs::File;
use std::future::Future;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Poll, Waker};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("syscall failed")]
    SyscallError(#[from] libredox::error::Error),
    #[error("the device is incapable of {0:?}")]
    InCapable(CfgType),
}

/// Returns the queue part sizes in bytes.
///
/// ## Reference
/// Section 2.7 Split Virtqueues of the specfication v1.2 describes the alignment
/// and size of the queue parts.
///
/// ## Panics
/// If `queue_size` is not a power of two or is zero.
pub const fn queue_part_sizes(queue_size: usize) -> (usize, usize, usize) {
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
        align(desc, DESCRIPTOR_ALIGN).next_multiple_of(syscall::PAGE_SIZE),
        align(avail, AVAILABLE_ALIGN).next_multiple_of(syscall::PAGE_SIZE),
        align(used, USED_ALIGN).next_multiple_of(syscall::PAGE_SIZE),
    )
}

pub fn spawn_irq_thread(irq_handle: &File, queue: &Arc<Queue<'static>>) {
    let irq_fd = irq_handle.as_raw_fd();
    let queue_copy = queue.clone();

    std::thread::spawn(move || {
        let event_queue = RawEventQueue::new().unwrap();

        event_queue
            .subscribe(irq_fd as usize, 0, event::EventFlags::READ)
            .unwrap();

        for event in event_queue.map(Result::unwrap) {
            // Wake up the tasks waiting on the queue.
            for (_, task) in queue_copy.waker.lock().unwrap().iter() {
                task.wake_by_ref();
            }
        }
    });
}

pub trait NotifyBell {
    fn ring(&self, queue_index: u16);
}

pub struct PendingRequest<'a> {
    queue: Arc<Queue<'a>>,
    first_descriptor: u32,
}

impl<'a> Future for PendingRequest<'a> {
    type Output = u32;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        // XXX: Register the waker before checking the queue to avoid the race condition
        //      where you lose a notification.
        self.queue
            .waker
            .lock()
            .unwrap()
            .insert(self.first_descriptor, cx.waker().clone());

        let used_head = self.queue.used.head_index();

        if used_head == self.queue.used_head.load(Ordering::SeqCst) {
            // No new requests have been completed.
            return Poll::Pending;
        }

        let used_element = self.queue.used.get_element_at((used_head - 1) as usize);
        let written = used_element.written.get();

        let mut table_index = used_element.table_index.get();

        if table_index == self.first_descriptor {
            // The request has been completed; recycle the descriptors used.
            while self.queue.descriptor[table_index as usize]
                .flags()
                .contains(DescriptorFlags::NEXT)
            {
                let next_index = self.queue.descriptor[table_index as usize].next();
                self.queue.descriptor_stack.push(table_index as u16);
                table_index = next_index.into();
            }

            // Push the last descriptor.
            self.queue.descriptor_stack.push(table_index as u16);
            self.queue
                .waker
                .lock()
                .unwrap()
                .remove(&self.first_descriptor);

            self.queue.used_head.store(used_head, Ordering::SeqCst);
            return Poll::Ready(written);
        } else {
            return Poll::Pending;
        }
    }
}

pub struct Queue<'a> {
    pub queue_index: u16,
    pub waker: Mutex<std::collections::HashMap<u32, Waker>>,
    pub used: Used<'a>,
    pub descriptor: Dma<[Descriptor]>,
    pub available: Available<'a>,
    pub used_head: AtomicU16,
    vector: u16,

    notification_bell: Box<dyn NotifyBell>,
    descriptor_stack: crossbeam_queue::SegQueue<u16>,
    sref: Weak<Self>,
}

impl<'a> Queue<'a> {
    pub fn new<N>(
        descriptor: Dma<[Descriptor]>,
        available: Available<'a>,
        used: Used<'a>,

        notification_bell: N,
        queue_index: u16,
        vector: u16,
    ) -> Arc<Self>
    where
        N: NotifyBell + 'static,
    {
        let descriptor_stack = crossbeam_queue::SegQueue::new();
        (0..descriptor.len() as u16).for_each(|i| descriptor_stack.push(i));

        Arc::new_cyclic(|sref| Self {
            notification_bell: Box::new(notification_bell),
            available,
            descriptor,
            used,
            waker: Mutex::new(std::collections::HashMap::new()),
            queue_index,
            descriptor_stack,
            used_head: AtomicU16::new(0),
            sref: sref.clone(),
            vector,
        })
    }

    fn reinit(&self) {
        self.used_head.store(0, Ordering::SeqCst);
        self.available.set_head_idx(0);

        // Drain all of the available descriptors.
        while let Some(_) = self.descriptor_stack.pop() {}

        // Refill the descriptor stack.
        (0..self.descriptor.len() as u16).for_each(|i| self.descriptor_stack.push(i));
    }

    #[must_use = "The function returns a future that must be awaited to ensure the sent request is completed."]
    pub fn send(&self, chain: Vec<Buffer>) -> PendingRequest<'a> {
        let mut first_descriptor: Option<usize> = None;
        let mut last_descriptor: Option<usize> = None;

        for buffer in chain.iter() {
            let descriptor = self.descriptor_stack.pop().unwrap() as usize;

            if first_descriptor.is_none() {
                first_descriptor = Some(descriptor);
            }

            self.descriptor[descriptor].set_addr(buffer.buffer as u64);
            self.descriptor[descriptor].set_flags(buffer.flags);
            self.descriptor[descriptor].set_size(buffer.size as u32);

            if let Some(index) = last_descriptor {
                self.descriptor[index].set_next(Some(descriptor as u16));
            }

            last_descriptor = Some(descriptor);
        }

        let last_descriptor = last_descriptor.unwrap();
        let first_descriptor = first_descriptor.unwrap();

        self.descriptor[last_descriptor].set_next(None);

        let index = self.available.head_index() as usize;

        self.available
            .get_element_at(index)
            .set_table_index(first_descriptor as u16);

        self.available.set_head_idx(index as u16 + 1);
        self.notification_bell.ring(self.queue_index);

        PendingRequest {
            queue: self.sref.upgrade().unwrap(),
            first_descriptor: first_descriptor as u32,
        }
    }

    /// Returns the number of descriptors in the descriptor table of this queue.
    pub fn descriptor_len(&self) -> usize {
        self.descriptor.len()
    }
}

unsafe impl Sync for Queue<'_> {}
unsafe impl Send for Queue<'_> {}

pub struct Available<'a> {
    mem: Mem<'a>,
    queue_size: usize,
}
pub struct Borrowed<'a> {
    phys: usize,
    virt: usize,
    size: usize,
    _unused: &'a (),
}
pub enum Mem<'a> {
    Owned(Dma<[u8]>),
    Borrowed(Borrowed<'a>),
}
impl Borrowed<'_> {
    pub unsafe fn new(phys: usize, virt: usize, size: usize) -> Self {
        Self {
            phys,
            virt,
            size,
            _unused: &(),
        }
    }
}
impl<'a> Mem<'a> {
    pub fn as_ptr<T>(&self) -> *const T {
        match *self {
            Self::Owned(ref dma) => dma.as_ptr().cast(),
            Self::Borrowed(Borrowed {
                phys: _,
                virt,
                size: _,
                _unused,
            }) => virt as *const T,
        }
    }
    pub fn as_mut_ptr<T>(&mut self) -> *mut T {
        match *self {
            Self::Owned(ref mut dma) => dma.as_mut_ptr().cast(),
            Self::Borrowed(Borrowed {
                phys: _,
                virt,
                size: _,
                _unused,
            }) => virt as *mut T,
        }
    }
    pub fn physical(&self) -> usize {
        match self {
            Self::Owned(dma) => dma.physical(),
            Self::Borrowed(borrowed) => borrowed.phys,
        }
    }
}

impl<'a> Available<'a> {
    pub fn ring(&self) -> &AvailableRing {
        unsafe { &*self.mem.as_ptr() }
    }
    pub fn ring_mut(&mut self) -> &mut AvailableRing {
        unsafe { &mut *self.mem.as_mut_ptr() }
    }
    pub fn new(queue_size: usize) -> Result<Self, Error> {
        let (_, _, size) = queue_part_sizes(queue_size);
        let mem = unsafe {
            Dma::zeroed_slice(size)
                .map_err(Error::SyscallError)?
                .assume_init()
        };

        unsafe { Self::from_raw(Mem::Owned(mem), queue_size) }
    }

    /// `addr` is the physical address of the ring.
    pub unsafe fn from_raw(mem: Mem<'a>, queue_size: usize) -> Result<Self, Error> {
        let ring = Self { mem, queue_size };

        for i in 0..queue_size {
            // Setting them to `u16::MAX` helps with debugging since qemu reports them
            // as illegal values.
            ring.get_element_at(i)
                .table_index
                .store(u16::MAX, Ordering::SeqCst);
        }

        Ok(ring)
    }

    /// ## Panics
    /// This function panics if the index is out of bounds.
    pub fn get_element_at(&self, index: usize) -> &AvailableRingElement {
        // SAFETY: We have exclusive access to the elements and the number of elements
        //         is correct; same as the queue size.
        unsafe {
            self.ring()
                .elements
                .as_slice(self.queue_size)
                .get(index % self.queue_size)
                .expect("virtio-core::available: index out of bounds")
        }
    }

    pub fn head_index(&self) -> u16 {
        self.ring().head_index.load(Ordering::SeqCst)
    }

    pub fn set_head_idx(&self, index: u16) {
        self.ring().head_index.store(index, Ordering::SeqCst);
    }

    pub fn phys_addr(&self) -> usize {
        self.mem.physical()
    }
}

impl<'a> Drop for Available<'a> {
    fn drop(&mut self) {
        log::warn!(
            "virtio-core: dropping 'available' ring at {:#x}",
            self.phys_addr()
        );
    }
}

pub struct Used<'a> {
    mem: Mem<'a>,
    queue_size: usize,
    _unused: &'a (),
}

impl<'a> Used<'a> {
    fn ring(&self) -> &UsedRing {
        unsafe { &*self.mem.as_ptr() }
    }
    fn ring_mut(&mut self) -> &mut UsedRing {
        unsafe { &mut *self.mem.as_mut_ptr() }
    }

    pub fn new(queue_size: usize) -> Result<Self, Error> {
        let (_, _, size) = queue_part_sizes(queue_size);
        let mem = unsafe {
            Dma::zeroed_slice(size)
                .map_err(Error::SyscallError)?
                .assume_init()
        };

        unsafe { Self::from_raw(Mem::Owned(mem), queue_size) }
    }

    /// `addr` is the physical address of the ring.
    pub unsafe fn from_raw(mem: Mem<'a>, queue_size: usize) -> Result<Self, Error> {
        let mut ring = Self {
            mem,
            queue_size,
            _unused: &(),
        };

        for i in 0..queue_size {
            // Setting them to `u32::MAX` helps with debugging since qemu reports them
            // as illegal values.
            ring.get_mut_element_at(i).table_index.set(u32::MAX);
        }

        Ok(ring)
    }

    /// ## Panics
    /// This function panics if the index is out of bounds.
    pub fn get_element_at(&self, index: usize) -> &UsedRingElement {
        // SAFETY: We have exclusive access to the elements and the number of elements
        //         is correct; same as the queue size.
        unsafe {
            self.ring()
                .elements
                .as_slice(self.queue_size)
                .get(index % self.queue_size)
                .expect("virtio-core::used: index out of bounds")
        }
    }

    /// ## Panics
    /// This function panics if the index is out of bounds.
    pub fn get_mut_element_at(&mut self, index: usize) -> &mut UsedRingElement {
        // SAFETY: We have exclusive access to the elements and the number of elements
        //         is correct; same as the queue size.
        let queue_size = self.queue_size;
        unsafe {
            self.ring_mut()
                .elements
                .as_mut_slice(queue_size)
                .get_mut(index % 256)
                .expect("virtio-core::used: index out of bounds")
        }
    }

    pub fn flags(&self) -> u16 {
        self.ring().flags.get()
    }

    pub fn head_index(&self) -> u16 {
        self.ring().head_index.get()
    }

    pub fn phys_addr(&self) -> usize {
        self.mem.physical()
    }
}

impl Drop for Used<'_> {
    fn drop(&mut self) {
        log::warn!(
            "virtio-core: dropping 'used' ring at {:#x}",
            self.phys_addr()
        );
    }
}

pub trait Transport: Sync + Send {
    /// `size` specifies the size of the read in bytes.
    ///
    /// ## Panics
    /// This function panics if the provided `size` is more then `size_of::<u64>()`.
    fn load_config(&self, offset: u8, size: u8) -> u64;

    /// Resets the device.
    fn reset(&self);

    /// Returns whether the device supports the specified feature.
    fn check_device_feature(&self, feature: u32) -> bool;

    /// Acknowledges the specified feature.
    ///
    /// **Note**: [`Transport::check_device_feature`] must be used to check whether
    /// the device supports the feature before acknowledging it.
    fn ack_driver_feature(&self, feature: u32);

    /// Finalizes the acknowledged features by setting the `FEATURES_OK` bit in the
    /// device status flags.
    fn finalize_features(&self);

    /// Runs the device.
    ///
    /// At this point, all of the queues must be created and the features must be
    /// finalized.
    ///
    /// ## Panics
    /// This function panics if the device is already running.
    fn run_device(&self) {
        self.insert_status(DeviceStatusFlags::DRIVER_OK);
    }

    /// Request to be notified on configuration changes on the given MSI-X vector.
    fn setup_config_notify(&self, vector: u16);

    /// Each time the device configuration changes this number will be updated.
    fn config_generation(&self) -> u32;

    /// Creates a new queue.
    ///
    /// ## Panics
    /// This function panics if the device is running.
    fn setup_queue(&self, vector: u16, irq_handle: &File) -> Result<Arc<Queue<'_>>, Error>;

    // TODO(andypython): Should this function be unsafe?
    fn reinit_queue(&self, queue: Arc<Queue>);
    fn insert_status(&self, status: DeviceStatusFlags);
}

struct StandardBell<'a>(&'a mut AtomicU16);

impl NotifyBell for StandardBell<'_> {
    #[inline]
    fn ring(&self, queue_index: u16) {
        self.0.store(queue_index, Ordering::SeqCst);
    }
}

pub struct StandardTransport<'a> {
    pub(crate) common: Mutex<&'a mut CommonCfg>,
    notify: *const u8,
    notify_mul: u32,
    device_space: *const u8,

    queue_index: AtomicU16,
}

impl<'a> StandardTransport<'a> {
    pub fn new(
        common: &'a mut CommonCfg,
        notify: *const u8,
        notify_mul: u32,
        device_space: *const u8,
    ) -> Arc<Self> {
        Arc::new(Self {
            common: Mutex::new(common),
            notify,
            notify_mul,

            queue_index: AtomicU16::new(0),
            device_space,
        })
    }
}

impl Transport for StandardTransport<'_> {
    fn load_config(&self, offset: u8, size: u8) -> u64 {
        unsafe {
            let ptr = self.device_space.add(offset as usize);
            let size = size as usize;

            if size == size_of::<u8>() {
                ptr.cast::<u8>().read() as u64
            } else if size == size_of::<u16>() {
                ptr.cast::<u16>().read() as u64
            } else if size == size_of::<u32>() {
                ptr.cast::<u32>().read() as u64
            } else if size == size_of::<u64>() {
                ptr.cast::<u64>().read() as u64
            } else {
                unreachable!()
            }
        }
    }

    fn reset(&self) {
        let mut common = self.common.lock().unwrap();

        common.device_status.set(DeviceStatusFlags::empty());
        // Upon reset, the device must initialize device status to 0.
        assert_eq!(common.device_status.get(), DeviceStatusFlags::empty());
    }

    fn check_device_feature(&self, feature: u32) -> bool {
        let mut common = self.common.lock().unwrap();

        common.device_feature_select.set(feature >> 5);
        (common.device_feature.get() & (1 << (feature & 31))) != 0
    }

    fn ack_driver_feature(&self, feature: u32) {
        let mut common = self.common.lock().unwrap();

        common.driver_feature_select.set(feature >> 5);

        let current = common.driver_feature.get();
        common.driver_feature.set(current | (1 << (feature & 31)));
    }

    fn finalize_features(&self) {
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

    fn setup_config_notify(&self, vector: u16) {
        self.common.lock().unwrap().config_msix_vector.set(vector);
    }

    fn config_generation(&self) -> u32 {
        u32::from(self.common.lock().unwrap().config_generation.get())
    }

    fn setup_queue(&self, vector: u16, irq_handle: &File) -> Result<Arc<Queue<'_>>, Error> {
        let mut common = self.common.lock().unwrap();

        let queue_index = self.queue_index.fetch_add(1, Ordering::SeqCst);
        common.queue_select.set(queue_index);

        let queue_size = common.queue_size.get() as usize;
        let queue_notify_idx = common.queue_notify_off.get();

        // Allocate memory for the queue structues.
        let descriptor = unsafe {
            Dma::<[Descriptor]>::zeroed_slice(queue_size)
                .map_err(Error::SyscallError)?
                .assume_init()
        };

        let avail = Available::new(queue_size)?;
        let used = Used::new(queue_size)?;

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
            &mut *(self.notify.add(offset as usize) as *mut AtomicU16)
        };

        log::info!("virtio-core: enabled queue #{queue_index} (size={queue_size})");

        let queue = Queue::new(
            descriptor,
            avail,
            used,
            StandardBell(notification_bell),
            queue_index,
            vector,
        );

        spawn_irq_thread(irq_handle, &queue);
        Ok(queue)
    }

    fn insert_status(&self, status: DeviceStatusFlags) {
        let mut common = self.common.lock().unwrap();
        let old = common.device_status.get();

        common.device_status.set(old | status);
    }

    /// Re-initializes a queue; usually done after a device reset.
    fn reinit_queue(&self, queue: Arc<Queue>) {
        let mut common = self.common.lock().unwrap();
        queue.reinit();

        common.queue_select.set(queue.queue_index);

        common.queue_desc.set(queue.descriptor.physical() as u64);
        common.queue_driver.set(queue.available.phys_addr() as u64);
        common.queue_device.set(queue.used.phys_addr() as u64);

        // Set the MSI-X vector.
        common.queue_msix_vector.set(queue.vector);
        assert!(common.queue_msix_vector.get() == queue.vector);

        // Enable the queue.
        common.queue_enable.set(1);
    }
}

unsafe impl Send for StandardTransport<'_> {}
unsafe impl Sync for StandardTransport<'_> {}
