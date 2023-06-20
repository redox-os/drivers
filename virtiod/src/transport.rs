use crate::*;
use pcid_interface::PciHeader;

pub struct StandardTransport<'a> {
    header: PciHeader,
    common: &'a mut CommonCfg,
}

impl<'a> StandardTransport<'a> {
    pub fn new(header: PciHeader, common: &'a mut CommonCfg) -> Self {
        Self { header, common }
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
}
