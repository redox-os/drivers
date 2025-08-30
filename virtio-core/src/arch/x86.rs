use crate::transport::Error;

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector_for_msi, read_bsp_apic_id};
use std::fs::File;

use crate::MSIX_PRIMARY_VECTOR;

use pcid_interface::*;

pub fn enable_msix(pcid_handle: &mut PciFunctionHandle) -> Result<File, Error> {
    // Extended message signaled interrupts.
    let msix_info = match pcid_handle.feature_info(PciFeature::MsiX) {
        PciFeatureInfo::MsiX(capability) => capability,
        _ => unreachable!(),
    };
    let mut info = unsafe { msix_info.map_and_mask_all(pcid_handle) };

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

    pcid_handle.enable_feature(PciFeature::MsiX);

    log::info!("virtio: using MSI-X (interrupt_handle={interrupt_handle:?})");
    Ok(interrupt_handle)
}
