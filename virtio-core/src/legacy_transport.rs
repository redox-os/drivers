use std::{sync::{Weak, atomic::{AtomicU16, Ordering}, Arc}, mem::size_of, fs::File};

use common::dma::Dma;
use syscall::{Pio, Io};

use crate::{transport::{NotifyBell, Transport, Queue, Error, Available, Used, queue_part_sizes, spawn_irq_thread, Mem, Borrowed}, spec::{Descriptor, DeviceStatusFlags}};


pub enum LegacyRegister {
    DeviceFeatures = 0, // u32

    QueueAddress = 8, // u32
    QueueSize = 12,   // u16
    QueueSelect = 14, // u16
    QueueNotify = 16, // u16

    DeviceStatus = 18, // u8

    ConfigMsixVector = 20, // u16
    QueueMsixVector = 22,  // u16
}

struct LegacyBell(Weak<LegacyTransport>);

impl NotifyBell for LegacyBell {
    #[inline]
    fn ring(&self, queue_index: u16) {
        let transport = self.0.upgrade().expect("bell: transport dropped");
        transport.write::<u16>(LegacyRegister::QueueNotify, queue_index)
    }
}

pub struct LegacyTransport(u16, AtomicU16, Weak<Self>);

impl LegacyTransport {
    pub(super) fn new(port: u16) -> Arc<Self> {
        Arc::new_cyclic(|sref| Self(port, AtomicU16::new(0), sref.clone()))
    }

    unsafe fn read_raw<V>(&self, offset: usize) -> V
    where
        V: Sized + TryFrom<u64>,
        <V as TryFrom<u64>>::Error: std::fmt::Debug,
    {
        let port = self.0 + offset as u16;

        if size_of::<V>() == size_of::<u8>() {
            V::try_from(Pio::<u8>::new(port).read() as u64).unwrap()
        } else if size_of::<V>() == size_of::<u16>() {
            V::try_from(Pio::<u16>::new(port).read() as u64).unwrap()
        } else if size_of::<V>() == size_of::<u32>() {
            V::try_from(Pio::<u32>::new(port).read() as u64).unwrap()
        } else if size_of::<V>() == size_of::<u64>() {
            let lower = Pio::<u32>::new(port).read() as u64;
            let upper = Pio::<u32>::new(port + size_of::<u32>() as u16).read() as u64;

            V::try_from(lower | (upper << 32)).unwrap()
        } else {
            unreachable!()
        }
    }

    fn read<V>(&self, register: LegacyRegister) -> V
    where
        V: Sized + TryFrom<u64>,
        <V as TryFrom<u64>>::Error: std::fmt::Debug,
    {
        unsafe { self.read_raw(register as usize) }
    }

    fn write<V>(&self, register: LegacyRegister, value: V)
    where
        V: Sized + TryInto<usize>,
        <V as TryInto<usize>>::Error: std::fmt::Debug,
    {
        if size_of::<V>() == size_of::<u8>() {
            Pio::<u8>::new(self.0 + register as u16).write(value.try_into().unwrap() as u8);
        } else if size_of::<V>() == size_of::<u16>() {
            Pio::<u16>::new(self.0 + register as u16).write(value.try_into().unwrap() as u16);
        } else if size_of::<V>() == size_of::<u32>() {
            Pio::<u32>::new(self.0 + register as u16).write(value.try_into().unwrap() as u32);
        } else {
            unreachable!()
        }
    }
}

impl Transport for LegacyTransport {
    fn reset(&self) {
        self.write(LegacyRegister::DeviceStatus, 0u8);

        let status = self.read::<u8>(LegacyRegister::DeviceStatus);
        assert_eq!(status, 0);
    }

    fn check_device_feature(&self, feature: u32) -> bool {
        assert!(
            feature < 32,
            "virtio: cannot query feature {feature} on a legacy device"
        );
        self.read::<u32>(LegacyRegister::DeviceFeatures) & (1 << feature) == (1 << feature)
    }

    fn ack_driver_feature(&self, feature: u32) {
        assert!(
            feature < 32,
            "virtio: cannot ack feature {feature} on a legacy device"
        );

        let current = self.read::<u32>(LegacyRegister::DeviceFeatures);
        self.write::<u32>(LegacyRegister::DeviceFeatures, current | (1 << feature));
    }

    fn setup_queue(&self, vector: u16, irq_handle: &File) -> Result<Arc<Queue>, Error> {
        let queue_index = self.1.fetch_add(1, Ordering::SeqCst);
        self.write(LegacyRegister::QueueSelect, queue_index);

        let queue_size = self.read::<u16>(LegacyRegister::QueueSize) as usize;
        let (desc_size, avail_size, used_size) = queue_part_sizes(queue_size);

        let descriptor = unsafe {
            Dma::<[Descriptor]>::zeroed_slice(queue_size)?.assume_init()
        };

        let avail_addr = descriptor.physical() + desc_size;
        let avail_virt = (descriptor.as_ptr() as usize) + desc_size;
        let avail = unsafe { Available::from_raw(Mem::Borrowed(Borrowed::new(avail_addr, avail_virt, avail_size)), queue_size)? };

        let used_addr = avail_addr + avail_size;
        let used_virt = avail_virt + desc_size;
        let used = unsafe { Used::from_raw(Mem::Borrowed(Borrowed::new(used_addr, used_virt, used_size)), queue_size)? };

        self.write::<u16>(LegacyRegister::QueueMsixVector, vector);
        self.write::<u32>(LegacyRegister::QueueAddress, (descriptor.physical() as u32) >> 12);

        log::info!("virtio-core: enabled queue #{queue_index} (size={queue_size})");

        let queue = Queue::new(
            descriptor,
            avail,
            used,
            LegacyBell(self.2.clone()),
            queue_index,
            vector,
        );

        spawn_irq_thread(irq_handle, &queue);
        Ok(queue)
    }

    fn load_config(&self, offset: u8, size: u8) -> u64 {
        // We always enable MSI-X. So, the device configuration space offset will
        // always be 0x18.
        //
        // Checkout 4.1.4.8 Legacy Interfaces: A Note on PCI Device Layout
        const DEVICE_SPACE_OFFSET: usize = 0x18;

        let size = size as usize;
        let offset = DEVICE_SPACE_OFFSET + offset as usize;

        unsafe {
            if size == size_of::<u8>() {
                self.read_raw::<u8>(offset) as u64
            } else if size == size_of::<u16>() {
                self.read_raw::<u16>(offset) as u64
            } else if size == size_of::<u32>() {
                self.read_raw::<u32>(offset) as u64
            } else if size == size_of::<u64>() {
                self.read_raw::<u64>(offset) as u64
            } else {
                unreachable!()
            }
        }
    }

    fn insert_status(&self, status: DeviceStatusFlags) {
        let old = self.read::<u8>(LegacyRegister::DeviceStatus);
        self.write(LegacyRegister::DeviceStatus, old | status.bits());
    }

    fn reinit_queue(&self, _queue: Arc<Queue>) {
        todo!()
    }

    // Legacy devices do not have the `FEATURES_OK` bit.
    fn finalize_features(&self) {}
}
