use core::ptr::NonNull;

use static_assertions::const_assert_eq;
use thiserror::Error;

use virtiod::transport::StandardTransport;
use virtiod::*;

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector, read_bsp_apic_id};
use pcid_interface::msi::x86_64 as x86_64_msix;
use pcid_interface::msi::x86_64::DeliveryMode;
use pcid_interface::msi::{MsixCapability, MsixTableEntry};
use pcid_interface::*;

use syscall::{Io, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};

// TODO(andypython):
//
//           cc 3.1.1 Driver Requirements: Device Initialization
//
// ================ Generic =================
//          * Reset the device. [done]
//          * Set the ACKNOWLEDGE status bit: the guest OS has noticed the device. [done]
//          * Set the DRIVER status bit: the guest OS knows how to drive the device. [done]
//          * setup interrupts [done]
// =============== Driver Specific===============
// Read device feature bits, and write the subset of feature bits understood by the OS and driver to the device. During this step the driver MAY read (but MUST NOT write) the device-specific configuration fields to check that it can support the device before accepting it.
// Set the FEATURES_OK status bit. The driver MUST NOT accept new feature bits after this step.
// Re-read device status to ensure the FEATURES_OK bit is still set: otherwise, the device does not support our subset of features and the device is unusable.
// Perform device-specific setup, including discovery of virtqueues for the device, optional per-bus setup, reading and possibly writing the device’s virtio configuration space, and population of virtqueues.
// Set the DRIVER_OK status bit. At this point the device is “live”.

use std::ops::{Add, Div, Rem};

fn div_round_up<T>(a: T, b: T) -> T
where
    T: Add<Output = T> + Div<Output = T> + Rem<Output = T> + PartialEq + From<u8> + Copy,
{
    if a % b != T::from(0u8) {
        a / b + T::from(1u8)
    } else {
        a / b
    }
}

pub fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "redox")]
    setup_logging();

    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}

#[cfg(target_os = "redox")]
fn setup_logging() {
    use redox_log::{OutputBuilder, RedoxLogger};

    let mut logger = RedoxLogger::new().with_output(
        OutputBuilder::stderr()
            // limit global output to important info
            .with_filter(log::LevelFilter::Info)
            .with_ansi_escape_codes()
            .flush_on_newline(true)
            .build(),
    );

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "virtiod.log") {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(log::LevelFilter::Info)
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(err) => eprintln!("virtiod: failed to create log: {}", err),
    }

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "virtiod.ansi.log") {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(log::LevelFilter::Info)
                    .with_ansi_escape_codes()
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(err) => eprintln!("virtiod: failed to create ansi log: {}", err),
    }

    logger.enable().unwrap();
    log::info!("virtiod: enabled logger");
}

struct MsixInfo {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub virt_pba_base: NonNull<u64>,
    pub capability: MsixCapability,
}

impl MsixInfo {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().offset(k as isize)
    }

    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

const_assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);

#[derive(Debug, Copy, Clone, Error)]
enum Error {
    #[error("capability {0:?} not found")]
    InCapable(CfgType),
    #[error("failed to map memory")]
    Physmap,
    #[error("failed to allocate an interrupt vector")]
    ExhaustedInt,
}

fn enable_msix(pcid_handle: &mut PcidServerHandle) -> anyhow::Result<()> {
    let pci_config = pcid_handle.fetch_config()?;

    // Extended message signaled interrupts.
    let capability = match pcid_handle.feature_info(PciFeature::MsiX)? {
        PciFeatureInfo::MsiX(capability) => capability,
        _ => unreachable!(),
    };

    let table_size = capability.table_size();
    let table_base = capability.table_base_pointer(pci_config.func.bars);
    let table_min_length = table_size * 16;
    let pba_min_length = div_round_up(table_size, 8);

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
        syscall::physmap(
            bar_ptr as usize,
            bar_size as usize,
            PHYSMAP_WRITE | PHYSMAP_NO_CACHE,
        )
        .map_err(|_| Error::Physmap)?
    };

    // Ensure that the table and PBA are be within the BAR.
    {
        let bar_range = bar_ptr..bar_ptr + bar_size;
        assert!(bar_range.contains(&(table_base as u64 + table_min_length as u64)));
        assert!(bar_range.contains(&(pba_base as u64 + pba_min_length as u64)));
    }

    let virt_table_base = ((table_base - bar_ptr as usize) + address) as *mut MsixTableEntry;
    let virt_pba_base = ((pba_base - bar_ptr as usize) + address) as *mut u64;

    let mut info = MsixInfo {
        virt_table_base: NonNull::new(virt_table_base).unwrap(),
        virt_pba_base: NonNull::new(virt_pba_base).unwrap(),
        capability,
    };

    // Allocate the primary MSI vector.
    let (vector, interrupt_handle) = {
        let k = 0;
        let table_entry_pointer = info.table_entry_pointer(k);

        let destination_id = read_bsp_apic_id()?;
        let lapic_id = u8::try_from(destination_id).unwrap();

        let rh = false;
        let dm = false;
        let addr = x86_64_msix::message_address(lapic_id, rh, dm);

        let (vector, interrupt_handle) =
            allocate_single_interrupt_vector(destination_id)?.ok_or(Error::ExhaustedInt)?;

        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

        table_entry_pointer.addr_lo.write(addr);
        table_entry_pointer.addr_hi.write(0);
        table_entry_pointer.msg_data.write(msg_data);
        table_entry_pointer
            .vec_ctl
            .writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);

        (vector, interrupt_handle)
    };

    pcid_handle.enable_feature(PciFeature::MsiX)?;

    log::info!("virtio: using MSI-X (vector={vector}, interrupt_handle={interrupt_handle:?})");
    Ok(())
}

fn deamon(_deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    let mut pcid_handle = PcidServerHandle::connect_default()?;
    let pci_config = pcid_handle.fetch_config()?;
    let pci_header = pcid_handle.fetch_header()?;

    // 0x1001 - virtio-blk
    assert_eq!(pci_config.func.devid, 0x1001);
    log::info!("virtiod: found `virtio-blk` device");

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
        assert!(capability.data.len() >= core::mem::size_of::<PciCapability>());

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
            syscall::physmap(
                addr + capability.offset as usize,
                capability.length as usize,
                PHYSMAP_WRITE | PHYSMAP_NO_CACHE,
            )
            .map_err(|_| Error::Physmap)?
        };

        match capability.cfg_type {
            CfgType::Common => {
                debug_assert!(common_addr.is_none());
                common_addr = Some(address);
            }

            CfgType::Notify => {
                debug_assert!(notify_addr.is_none());
                notify_addr = Some(address);
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

        log::info!("virtio: {capability:?}");
    }

    let common_addr = common_addr.ok_or(Error::InCapable(CfgType::Common))?;
    let notify_addr = notify_addr.ok_or(Error::InCapable(CfgType::Notify))?;
    let isr_addr = isr_addr.ok_or(Error::InCapable(CfgType::Isr))?;
    let device_addr = device_addr.ok_or(Error::InCapable(CfgType::Device))?;

    let common = unsafe { &mut *(common_addr as *mut CommonCfg) };

    // Reset the device.
    common.device_status.set(DeviceStatusFlags::empty());
    // Upon reset, the device must initialize device status to 0.
    assert_eq!(common.device_status.get(), DeviceStatusFlags::empty());
    log::info!("virtio: successfully reseted the device");

    // XXX: According to the virtio specification v1.2, setting the ACKNOWLEDGE and DRIVER bits
    //      in `device_status` are required to be done in two steps.
    common
        .device_status
        .set(common.device_status.get() | DeviceStatusFlags::ACKNOWLEDGE);

    common
        .device_status
        .set(common.device_status.get() | DeviceStatusFlags::DRIVER);

    // Setup interrupts.
    let all_pci_features = pcid_handle.fetch_all_features()?;
    let has_msix = all_pci_features
        .iter()
        .any(|(feature, _)| feature.is_msix());

    // According to the virtio specification, the device REQUIRED to support MSI-X.
    assert!(has_msix, "virtio: device does not support MSI-X");
    enable_msix(&mut pcid_handle)?;

    log::info!("virtio: using standard PCI transport");

    let mut transport = StandardTransport::new(pci_header, common);

    // Check VirtIO version 1 compliance.
    assert!(transport.check_device_feature(VIRTIO_F_VERSION_1));
    transport.ack_driver_feature(VIRTIO_F_VERSION_1);

    loop {}
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}
