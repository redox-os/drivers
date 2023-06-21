#![deny(trivial_numeric_casts, unused_allocation)]

use core::ptr::NonNull;

use std::fs::File;
use std::io::{self, ErrorKind};
use std::io::{Read, Write};
use std::ops::{Add, Div, Rem};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use static_assertions::const_assert_eq;

use virtiod::transport::StandardTransport;
use virtiod::*;

use pcid_interface::irq_helpers::{allocate_single_interrupt_vector, read_bsp_apic_id};
use pcid_interface::msi::x86_64 as x86_64_msix;
use pcid_interface::msi::x86_64::DeliveryMode;
use pcid_interface::msi::{MsixCapability, MsixTableEntry};
use pcid_interface::*;

use event::EventQueue;
use syscall::{Io, Packet, SchemeBlockMut, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};

use virtiod::utils::VolatileCell;

mod scheme;

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
            .with_filter(log::LevelFilter::Trace)
            .with_ansi_escape_codes()
            .flush_on_newline(true)
            .build(),
    );

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "virtiod.log") {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(log::LevelFilter::Trace)
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
                    .with_filter(log::LevelFilter::Trace)
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
        &mut *self.virt_table_base.as_ptr().add(k)
    }

    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

const_assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);

const MSIX_PRIMARY_VECTOR: u16 = 0;

fn enable_msix(pcid_handle: &mut PcidServerHandle) -> anyhow::Result<File> {
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
    let interrupt_handle = {
        let table_entry_pointer = info.table_entry_pointer(MSIX_PRIMARY_VECTOR as usize);

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

        interrupt_handle
    };

    pcid_handle.enable_feature(PciFeature::MsiX)?;

    log::info!("virtio: using MSI-X (interrupt_handle={interrupt_handle:?})");
    Ok(interrupt_handle)
}

#[repr(C)]
pub struct BlockGeometry {
    pub cylinders: VolatileCell<u16>,
    pub heads: VolatileCell<u8>,
    pub sectors: VolatileCell<u8>,
}

#[repr(C)]
pub struct BlockDeviceConfig {
    capacity: VolatileCell<u64>,
    pub size_max: VolatileCell<u32>,
    pub seq_max: VolatileCell<u32>,
    pub geometry: BlockGeometry,
    blk_size: VolatileCell<u32>,
}

impl BlockDeviceConfig {
    /// Returns the capacity of the block device in bytes.
    pub fn capacity(&self) -> u64 {
        self.capacity.get()
    }

    pub fn block_size(&self) -> u32 {
        self.blk_size.get()
    }
}

#[repr(u32)]
pub enum BlockRequestTy {
    In = 0,
    Out = 1,
}

const_assert_eq!(core::mem::size_of::<BlockRequestTy>(), 4);

#[repr(C)]
pub struct BlockVirtRequest {
    pub ty: BlockRequestTy,
    pub reserved: u32,
    pub sector: u64,
}

const_assert_eq!(core::mem::size_of::<BlockVirtRequest>(), 16);

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
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

        log::info!("virtio: {capability:?}");
    }

    let common_addr = common_addr.ok_or(Error::InCapable(CfgType::Common))?;
    let (notify_addr, notify_multiplier) = notify_addr.ok_or(Error::InCapable(CfgType::Notify))?;
    let isr_addr = isr_addr.ok_or(Error::InCapable(CfgType::Isr))?;
    let device_addr = device_addr.ok_or(Error::InCapable(CfgType::Device))?;

    assert!(
        notify_multiplier != 0,
        "virtio: device uses the same Queue Notify addresses for all queues"
    );

    let common = unsafe { &mut *(common_addr as *mut CommonCfg) };
    let device_space = unsafe { &mut *(device_addr as *mut BlockDeviceConfig) };
    let isr = unsafe { &*(isr_addr as *mut VolatileCell<u32>) };

    // Reset the device.
    common.device_status.set(DeviceStatusFlags::empty());
    // Upon reset, the device must initialize device status to 0.
    assert_eq!(common.device_status.get(), DeviceStatusFlags::empty());
    log::info!("virtio: successfully reseted the device");

    // XXX: According to the virtio specification v1.2, setting the ACKNOWLEDGE and DRIVER bits
    //      in `device_status` is required to be done in two steps.
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
    let mut irq_handle = enable_msix(&mut pcid_handle)?;

    log::info!("virtio: using standard PCI transport");

    let transport = StandardTransport::new(
        pci_header,
        common,
        notify_addr as *const u8,
        notify_multiplier,
    );
    transport.finalize_features();

    let queue = transport.setup_queue(MSIX_PRIMARY_VECTOR)?;
    let queue_copy = queue.clone();

    std::thread::spawn(move || {
        let mut event_queue = EventQueue::<usize>::new().unwrap();
        let mut progress_head = 0;

        event_queue
            .add(
                irq_handle.as_raw_fd(),
                move |_| -> Result<Option<usize>, io::Error> {
                    let isr = isr.get() as usize;

                    let mut inner = queue_copy.inner.lock().unwrap();
                    let used_head = inner.used.head_index();

                    if progress_head == used_head {
                        return Ok(None);
                    }

                    let used = inner.used.get_element_at((used_head - 1) as usize);
                    let mut desc_idx = used.table_index.get();
                    inner.descriptor_stack.push_back(desc_idx as u16);

                    loop {
                        let desc = &inner.descriptor[desc_idx as usize];
                        if !desc.flags.contains(DescriptorFlags::NEXT) {
                            break;
                        }

                        desc_idx = desc.next.into();
                        inner.descriptor_stack.push_back(desc_idx as u16);
                    }

                    progress_head = used_head;
                    drop(inner);

                    let mut buf = [0u8; 8];
                    irq_handle.read(&mut buf)?;
                    // Acknowledge the interrupt.
                    // irq_handle.write(&buf)?;
                    Ok(Some(isr))
                },
            )
            .unwrap();

        loop {
            event_queue.run().unwrap();
        }
    });

    // At this point the device is alive!
    transport.run_device();

    log::info!(
        "virtio-blk: disk size: {} sectors and block size of {} bytes",
        device_space.capacity.get(),
        device_space.blk_size.get()
    );

    let mut name = pci_config.func.name();
    name.push_str("_virtio_blk");

    let scheme_name = format!("disk/{}", name);

    let socket_fd = syscall::open(
        &format!(":{}", scheme_name),
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_CLOEXEC,
    )
    .map_err(Error::SyscallError)?;

    let mut socket_file = unsafe { File::from_raw_fd(socket_fd as RawFd) };

    let mut scheme = scheme::DiskScheme::new(scheme_name, queue, device_space);

    deamon.ready().expect("virtio: failed to deamonize");

    loop {
        let mut packet = Packet::default();
        socket_file
            .read(&mut packet)
            .expect("ahcid: failed to read disk scheme");
        let packey = scheme.handle(&mut packet);
        packet.a = packey.unwrap();
        socket_file
            .write(&mut packet)
            .expect("ahcid: failed to read disk scheme");
    }

    // for _ in 0..3 {
    //     let req = syscall::Dma::new(BlockVirtRequest {
    //         ty: BlockRequestTy::In,
    //         reserved: 0,
    //         sector: 0,
    //     })
    //     .unwrap();

    //     let result = syscall::Dma::new([0u8; 512]).unwrap();
    //     let status = syscall::Dma::new(u8::MAX).unwrap();

    //     let chain = ChainBuilder::new()
    //         .chain(Buffer::new(&req).flags(DescriptorFlags::NEXT))
    //         .chain(Buffer::new(&result).flags(DescriptorFlags::WRITE_ONLY | DescriptorFlags::NEXT))
    //         .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
    //         .build();

    //     queue.send(chain);

    //     log::info!("{}", event_queue.run()?);
    //     log::info!("command status: {}", *status);
    //     log::info!("data: {:?}", result.as_ref());
    // }
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}
