use crate::transport::Error;

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector_for_msi, read_bsp_apic_id};
use pcid_interface::msi::MsixTableEntry;
use std::{fs::File, ptr::NonNull};

use crate::{probe::MappedMsixRegs, MSIX_PRIMARY_VECTOR};

use pcid_interface::*;

pub fn enable_msix(pcid_handle: &mut PciFunctionHandle) -> Result<File, Error> {
    let pci_config = pcid_handle.config();

    // Extended message signaled interrupts.
    let msix_info = match pcid_handle.feature_info(PciFeature::MsiX)? {
        PciFeatureInfo::MsiX(capability) => capability,
        _ => unreachable!(),
    };
    msix_info.validate(pci_config.func.bars);

    let bar_address = unsafe { pcid_handle.map_bar(msix_info.table_bar)? }
        .ptr
        .as_ptr() as usize;
    let virt_table_base = (bar_address + msix_info.table_offset as usize) as *mut MsixTableEntry;

    let mut info = MappedMsixRegs {
        virt_table_base: NonNull::new(virt_table_base).unwrap(),
        info: msix_info,
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
