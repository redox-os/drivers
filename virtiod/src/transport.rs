use pcid_interface::PciHeader;

use crate::CommonCfg;

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
}
