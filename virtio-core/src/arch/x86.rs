use crate::{legacy_transport::LegacyTransport, reinit, transport::Error, Device};
use std::fs::File;

use pcid_interface::*;

pub fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {
    panic!("virtio-core: x86 doesn't support enable_msix")
}

pub fn probe_legacy_port_transport(
    pci_header: &PciHeader,
    pcid_handle: &mut PcidServerHandle,
) -> Result<Device, Error> {
    if let PciBar::Port(port) = pci_header.get_bar(0) {
        unsafe { syscall::iopl(3).expect("virtio: failed to set I/O privilege level") };
        log::warn!("virtio: using legacy transport");

        let transport = LegacyTransport::new(port);

        // Setup interrupts.
        let all_pci_features = pcid_handle.fetch_all_features()?;
        let has_msix = all_pci_features
            .iter()
            .any(|(feature, _)| feature.is_msix());

        // According to the virtio specification, the device REQUIRED to support MSI-X.
        assert!(has_msix, "virtio: device does not support MSI-X");
        let irq_handle = enable_msix(pcid_handle)?;

        let device = Device {
            transport,
            irq_handle,
            device_space: core::ptr::null_mut(),
        };

        device.transport.reset();
        reinit(&device)?;

        Ok(device)
    } else {
        unreachable!("virtio: legacy transport with non-port IO?")
    }
}
