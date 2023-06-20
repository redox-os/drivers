use crate::utils::align;
use crate::*;

use pcid_interface::PciHeader;
use syscall::{Dma, PhysBox};

use core::sync::atomic::{AtomicU16, Ordering};

pub struct StandardTransport<'a> {
    header: PciHeader,
    common: &'a mut CommonCfg,

    queue_index: AtomicU16,
}

impl<'a> StandardTransport<'a> {
    pub fn new(header: PciHeader, common: &'a mut CommonCfg) -> Self {
        Self {
            header,
            common,

            queue_index: AtomicU16::new(0),
        }
    }

    pub fn check_device_feature(&mut self, feature: u32) -> bool {
        self.common.device_feature_select.set(feature >> 5);
        (self.common.device_feature.get() & (1 << (feature & 31))) != 0
    }

    pub fn ack_driver_feature(&mut self, feature: u32) {
        self.common.driver_feature_select.set(feature >> 5);

        let current = self.common.driver_feature.get();
        self.common
            .driver_feature
            .set(current | (1 << (feature & 31)));
    }

    pub fn finalize_features(&mut self) {
        // Check VirtIO version 1 compliance.
        assert!(self.check_device_feature(VIRTIO_F_VERSION_1));
        self.ack_driver_feature(VIRTIO_F_VERSION_1);

        self.common
            .device_status
            .set(self.common.device_status.get() | DeviceStatusFlags::FEATURES_OK);

        // Re-read device status to ensure the `FEATURES_OK` bit is still set: otherwise,
        // the device does not support our subset of features and the device is unusable.
        let confirm = self.common.device_status.get();
        assert!((confirm & DeviceStatusFlags::FEATURES_OK) == DeviceStatusFlags::FEATURES_OK);
    }

    pub fn setup_queue(&mut self, vector: u16) -> anyhow::Result<()> {
        let queue_index = self.queue_index.fetch_add(1, Ordering::SeqCst);
        self.common.queue_select.set(queue_index);

        let queue_size = self.common.queue_size.get() as usize;
        let queue_notify_idx = self.common.queue_notify_off.get();

        assert!(queue_size != 0 && queue_size.is_power_of_two());

        // Get the queue size in bytes.
        //
        // Section 2.7 Split Virtqueues of the specfication describe the alignment
        // and size of the queues.
        const AVAILABLE_ALIGN: usize = 2;
        const USED_ALIGN: usize = 4;

        let table_size: usize = align(
            (queue_size as usize) * core::mem::size_of::<Descriptor>(),
            AVAILABLE_ALIGN,
        );

        let available_size = align(
            (queue_size as usize * core::mem::size_of::<AvailableRingElement>())
                + core::mem::size_of::<AvailableRingExtra>(),
            USED_ALIGN,
        );

        let used_size = (queue_size as usize) * core::mem::size_of::<UsedRingElement>()
            + core::mem::size_of::<UsedRingExtra>();

        // Allocate memory for the queue structues.
        let table = unsafe {
            Dma::<[Descriptor]>::zeroed_unsized(table_size)
                .map_err(|err| Error::SyscallError(err))?
        };

        let avaliable = unsafe {
            Dma::<[AvailableRing]>::zeroed_unsized(available_size)
                .map_err(|err| Error::SyscallError(err))?
        };

        let used = unsafe {
            Dma::<[UsedRing]>::zeroed_unsized(used_size).map_err(|err| Error::SyscallError(err))?
        };

        self.common.queue_desc.set(table.physical() as u64);
        self.common.queue_driver.set(avaliable.physical() as u64);
        self.common.queue_device.set(used.physical() as u64);

        // Set the MSI-X vector.
        self.common.queue_msix_vector.set(vector);
        assert!(self.common.queue_msix_vector.get() != 0);

        // Enable the queue.
        self.common.queue_enable.set(1);

        log::info!("virtio: enabled queue #{queue_index} (size={queue_size})");
        Ok(())
    }
}
