use pcid_interface::{Capability, PciBar, PcidServerHandle};
use thiserror::Error;
use virtiod::*;

use syscall::{PHYSMAP_NO_CACHE, PHYSMAP_WRITE};

// TODO(andypython):
//
//  cc 3.1.1 Driver Requirements: Device Initialization
// Reset the device.
// Set the ACKNOWLEDGE status bit: the guest OS has noticed the device.
// Set the DRIVER status bit: the guest OS knows how to drive the device.
// Read device feature bits, and write the subset of feature bits understood by the OS and driver to the device. During this step the driver MAY read (but MUST NOT write) the device-specific configuration fields to check that it can support the device before accepting it.
// Set the FEATURES_OK status bit. The driver MUST NOT accept new feature bits after this step.
// Re-read device status to ensure the FEATURES_OK bit is still set: otherwise, the device does not support our subset of features and the device is unusable.
// Perform device-specific setup, including discovery of virtqueues for the device, optional per-bus setup, reading and possibly writing the device’s virtio configuration space, and population of virtqueues.
// Set the DRIVER_OK status bit. At this point the device is “live”.

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

#[derive(Debug, Copy, Clone, Error)]
enum Error {
    #[error("capability {0:?} not found")]
    InCapable(CfgType),
    #[error("failed to map memory")]
    MapErr,
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
            .map_err(|_| Error::MapErr)?
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

    log::info!("virtio: using standard PCI transport");

    let _transport = StandardTransport::new();
    loop {}
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}
