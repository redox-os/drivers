use crate::{legacy_transport::LegacyTransport, reinit, transport::Error, Device};

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector_for_msi, read_bsp_apic_id};
use pcid_interface::msi::MsixTableEntry;
use std::{fs::File, ptr::NonNull};

use crate::{probe::MsixInfo, MSIX_PRIMARY_VECTOR};

use pcid_interface::*;

pub fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {
    let pci_config = pcid_handle.fetch_config()?;

    // Extended message signaled interrupts.
    let capability = match pcid_handle.feature_info(PciFeature::MsiX)? {
        PciFeatureInfo::MsiX(capability) => capability,
        _ => unreachable!(),
    };
    capability.validate(pci_config.func.bars);

    let bar = &pci_config.func.bars[capability.table_bir() as usize];
    let bar_address = unsafe { bar.physmap_mem("virtio-core") } as usize;
    let virt_table_base = (bar_address + capability.table_offset() as usize) as *mut MsixTableEntry;

    let mut info = MsixInfo {
        virt_table_base: NonNull::new(virt_table_base).unwrap(),
        capability,
    };

    // Allocate the primary MSI vector.
    // FIXME allow the driver to register multiple MSI-X vectors
    // FIXME move this MSI-X registering code into pcid_interface or pcid itself
    let interrupt_handle = {
        let table_entry_pointer = info.table_entry_pointer(MSIX_PRIMARY_VECTOR as usize);

        let destination_id = read_bsp_apic_id().expect("virtio_core: `read_bsp_apic_id()` failed");
        let (msg_addr_and_data, interrupt_handle) =
            allocate_single_interrupt_vector_for_msi(destination_id);
        table_entry_pointer.write_addr_and_data(msg_addr_and_data);
        table_entry_pointer.unmask();

        interrupt_handle
    };

    pcid_handle.enable_feature(PciFeature::MsiX)?;

    log::info!("virtio: using MSI-X (interrupt_handle={interrupt_handle:?})");
    Ok(interrupt_handle)
}

pub fn probe_legacy_port_transport(
    pci_config: &SubdriverArguments,
    pcid_handle: &mut PcidServerHandle,
) -> Result<Device, Error> {
    let port = pci_config.func.bars[0].expect_port();

    common::acquire_port_io_rights().expect("virtio: failed to set I/O privilege level");
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
}
