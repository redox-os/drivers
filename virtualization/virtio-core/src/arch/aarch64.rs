use std::fs::File;

use pcid_interface::*;

use crate::{transport::Error, Device};

pub fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {
    unimplemented!("virtio_core: aarch64 enable_msix")
}

pub fn probe_legacy_port_transport<'a>(
    pci_header: &PciHeader,
    pcid_handle: &mut PcidServerHandle,
) -> Result<Device<'a>, Error> {
    panic!("virtio-core: aarch64 doesn't support legacy port I/O")
}