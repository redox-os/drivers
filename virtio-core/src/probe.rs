use std::fs::File;
use std::ptr::NonNull;
use std::sync::Arc;

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector, read_bsp_apic_id};
use pcid_interface::msi::x86_64 as x86_64_msix;
use pcid_interface::msi::x86_64::DeliveryMode;
use pcid_interface::msi::{MsixCapability, MsixTableEntry};
use pcid_interface::*;

use syscall::Io;

use crate::spec::*;
use crate::transport::{Error, LegacyTransport, StandardTransport, Transport};
use crate::utils::{align_down, VolatileCell};

pub struct Device<'a> {
    pub transport: Arc<dyn Transport>,
    pub device_space: *const u8,
    pub irq_handle: File,
    pub isr: &'a VolatileCell<u32>,
}

// FIXME(andypython): `device_space` should not be `Send` nor `Sync`. Take
// it out of `Device`.
unsafe impl Send for Device<'_> {}
unsafe impl Sync for Device<'_> {}

struct MsixInfo {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub capability: MsixCapability,
}

impl MsixInfo {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().add(k)
    }

    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

static_assertions::const_assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);

pub const MSIX_PRIMARY_VECTOR: u16 = 0;

fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {
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
        let addr = x86_64_msix::message_address(lapic_id, rh, dm);

        let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id)
            .unwrap()
            .expect("virtio_core: interrupt vector exhaustion");

        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

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

/// VirtIO Device Probe
///
/// ## Device State
/// After this function, the device will have been successfully reseted and is ready for use.
///
/// The caller is required to do the following:
/// * Negotiate the device and driver supported features (finialize via [`StandardTransport::finalize_features`])
/// * Create the device specific virtio queues (via [`StandardTransport::setup_queue`]). This is *required* to be done
///   before starting the device.
/// * Finally start the device (via [`StandardTransport::run_device`]). At this point, the device
///   is alive.
///
/// ## Panics
/// This function panics if the device is not a virtio device.
pub fn probe_device<'a>(pcid_handle: &mut PcidServerHandle) -> Result<Device<'a>, Error> {
    let pci_config = pcid_handle.fetch_config()?;
    let pci_header = pcid_handle.fetch_header()?;

    assert_eq!(
        pci_config.func.venid, 6900,
        "virtio_core::probe_device: not a virtio device"
    );

    let mut common_addr = None;
    let mut notify_addr = None;
    let mut isr_addr = None;
    let mut device_addr = None;

    for capability in pcid_handle
        .get_capabilities()?
        .iter()
        .filter_map(|capability| {
            if let Capability::Vendor(vendor) = capability {
                Some(vendor)
            } else {
                None
            }
        })
    {
        // SAFETY: We have verified that the length of the data is correct.
        let capability = unsafe { &*(capability.data.as_ptr() as *const PciCapability) };

        match capability.cfg_type {
            CfgType::Common | CfgType::Notify | CfgType::Isr | CfgType::Device => {}
            _ => continue,
        }

        let bar = pci_header.get_bar(capability.bar as usize);
        let addr = match bar {
            PciBar::Memory32(addr) => addr as usize,
            PciBar::Memory64(addr) => addr as usize,

            _ => unreachable!("virtio: unsupported bar type: {bar:?}"),
        };

        let address = unsafe {
            let addr = addr + capability.offset as usize;

            // XXX: physmap() requires the address to be page aligned.
            let aligned_addr = align_down(addr);
            let offset = addr - aligned_addr;

            let size = offset + capability.length as usize;

            let addr = common::physmap(
                aligned_addr,
                size,
                common::Prot::RW,
                common::MemoryType::Uncacheable,
            )? as usize;

            addr + offset
        };

        match capability.cfg_type {
            CfgType::Common => {
                debug_assert!(common_addr.is_none());
                common_addr = Some(address);
            }

            CfgType::Notify => {
                debug_assert!(notify_addr.is_none());

                // SAFETY: The capability type is `Notify`, so its safe to access
                //         the `notify_multiplier` field.
                let multiplier = unsafe { capability.notify_multiplier() };
                notify_addr = Some((address, multiplier));
            }

            CfgType::Isr => {
                debug_assert!(isr_addr.is_none());
                isr_addr = Some(address);
            }

            CfgType::Device => {
                debug_assert!(device_addr.is_none());
                device_addr = Some(address);
            }

            _ => unreachable!(),
        }

        log::trace!("virtio-core::device-probe: {capability:?}");
    }

    if let (
        Some(common_addr),
        Some(isr_addr),
        Some(device_addr),
        Some((notify_addr, notify_multiplier)),
    ) = (common_addr, isr_addr, device_addr, notify_addr)
    {
        assert!(
            notify_multiplier != 0,
            "virtio-core::device_probe: device uses the same Queue Notify addresses for all queues"
        );

        let common = unsafe { &mut *(common_addr as *mut CommonCfg) };
        let device_space = unsafe { &mut *(device_addr as *mut u8) };
        let isr = unsafe { &*(isr_addr as *mut VolatileCell<u32>) };

        let transport = StandardTransport::new(
            common,
            notify_addr as *const u8,
            notify_multiplier,
            device_space,
        );

        // Setup interrupts.
        let all_pci_features = pcid_handle.fetch_all_features()?;
        let has_msix = all_pci_features
            .iter()
            .any(|(feature, _)| feature.is_msix());

        // According to the virtio specification, the device REQUIRED to support MSI-X.
        assert!(has_msix, "virtio: device does not support MSI-X");
        let irq_handle = enable_msix(pcid_handle)?;

        log::info!("virtio: using standard PCI transport");

        let device = Device {
            transport,
            device_space,
            irq_handle,
            isr,
        };

        device.transport.reset();
        reinit(&device)?;

        Ok(device)
    } else {
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
}

pub fn reinit<'a>(device: &Device<'a>) -> Result<(), Error> {
    // XXX: According to the virtio specification v1.2, setting the ACKNOWLEDGE and DRIVER bits
    //      in `device_status` is required to be done in two steps.
    device
        .transport
        .insert_status(DeviceStatusFlags::ACKNOWLEDGE);

    device.transport.insert_status(DeviceStatusFlags::DRIVER);
    Ok(())
}
