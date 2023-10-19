use crate::{
    reinit,
    transport::{Error},
    utils::VolatileCell,
    Device, legacy_transport::LegacyTransport,
};

use pcid_interface::msi::{self, MsixTableEntry};
use pcid_interface::irq_helpers::{allocate_single_interrupt_vector, read_bsp_apic_id};
use std::{ptr::NonNull, fs::File};

use syscall::Io;

use crate::{probe::MsixInfo, MSIX_PRIMARY_VECTOR};

use pcid_interface::*;

pub fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {

    let pci_config = pcid_handle.fetch_config()?;

    // Extended message signaled interrupts.
    let capability = match pcid_handle.feature_info(PciFeature::MsiX)? {
        PciFeatureInfo::MsiX(capability) => capability,
        _ => unreachable!(),
    };

    let table_size = capability.table_size();
    let table_base = capability.table_base_pointer(pci_config.func.bars);
    let table_min_length = table_size * 16;
    let pba_min_length = table_size.div_ceil(8);

    let pba_base = capability.pba_base_pointer(pci_config.func.bars);

    let bir = capability.table_bir() as usize;
    let bar = pci_config.func.bars[bir];
    let bar_size = pci_config.func.bar_sizes[bir] as u64;

    let bar_ptr = match bar {
        PciBar::Memory32(ptr) => ptr.into(),
        PciBar::Memory64(ptr) => ptr,
        _ => unreachable!(),
    };

    let address = unsafe {
        common::physmap(
            bar_ptr as usize,
            bar_size as usize,
            common::Prot::RW,
            common::MemoryType::Uncacheable,
        )? as usize
    };

    // Ensure that the table and PBA are be within the BAR.
    {
        let bar_range = bar_ptr..bar_ptr + bar_size;
        assert!(bar_range.contains(&(table_base as u64 + table_min_length as u64)));
        assert!(bar_range.contains(&(pba_base as u64 + pba_min_length as u64)));
    }

    let virt_table_base = ((table_base - bar_ptr as usize) + address) as *mut MsixTableEntry;

    let mut info = MsixInfo {
        virt_table_base: NonNull::new(virt_table_base).unwrap(),
        capability,
    };

    // Allocate the primary MSI vector.
    let interrupt_handle = {
        let table_entry_pointer = info.table_entry_pointer(MSIX_PRIMARY_VECTOR as usize);

        let destination_id = read_bsp_apic_id().expect("virtio_core: `read_bsp_apic_id()` failed");
        let lapic_id = u8::try_from(destination_id).unwrap();

        let rh = false;
        let dm = false;
        let addr = msi::x86_64::message_address(lapic_id, rh, dm);

        let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id)
            .unwrap()
            .expect("virtio_core: interrupt vector exhaustion");

        let msg_data = msi::x86_64::message_data_edge_triggered(msi::x86_64::DeliveryMode::Fixed, vector);

        table_entry_pointer.addr_lo.write(addr);
        table_entry_pointer.addr_hi.write(0);
        table_entry_pointer.msg_data.write(msg_data);
        table_entry_pointer
            .vec_ctl
            .writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);

        interrupt_handle
    };

    pcid_handle.enable_feature(PciFeature::MsiX)?;

    log::info!("virtio: using MSI-X (interrupt_handle={interrupt_handle:?})");
    Ok(interrupt_handle)
}

pub fn probe_legacy_port_transport<'a>(
    pci_header: &PciHeader,
    pcid_handle: &mut PcidServerHandle,
) -> Result<Device<'a>, Error> {
    if let PciBar::Port(port) = pci_header.get_bar(0) {
        unsafe { syscall::iopl(3).expect("virtio: failed to set I/O privilege level") };
        log::warn!("virtio: using legacy transport");

        static SHIM: VolatileCell<u32> = VolatileCell::new(0);

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
            isr: &SHIM,
            device_space: core::ptr::null_mut(),
        };

        device.transport.reset();
        reinit(&device)?;

        Ok(device)
    } else {
        unreachable!("virtio: legacy transport with non-port IO?")
    }
}
