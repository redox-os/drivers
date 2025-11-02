#![deny(trivial_numeric_casts, unused_allocation)]

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use driver_block::DiskScheme;
use static_assertions::const_assert_eq;

use pcid_interface::*;
use virtio_core::spec::*;

use virtio_core::transport::Transport;
use virtio_core::utils::VolatileCell;

mod scheme;

use thiserror::Error;

use crate::scheme::VirtioDisk;

#[derive(Debug, Error)]
pub enum Error {
    #[error("capability {0:?} not found")]
    InCapable(CfgType),
    #[error("failed to map memory")]
    Physmap,
    #[error("failed to allocate an interrupt vector")]
    ExhaustedInt,
    #[error("syscall error")]
    SyscallError(syscall::Error),
}

pub fn main() -> anyhow::Result<()> {
    common::setup_logging(
        "disk",
        "pci",
        "virtio-blkd",
        common::output_level(),
        common::file_level(),
    );
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}

#[repr(C)]
pub struct BlockGeometry {
    pub cylinders: VolatileCell<u16>,
    pub heads: VolatileCell<u8>,
    pub sectors: VolatileCell<u8>,
}

#[repr(u8)]
pub enum DeviceConfigTy {
    Capacity = 0,
    SizeMax = 0x8,
    SeqMax = 0xc,
    Geometry = 0x10,
    BlkSize = 0x14,
}

pub struct BlockDeviceConfig(Weak<dyn Transport>);

impl BlockDeviceConfig {
    #[inline]
    fn new(tranport: &Arc<dyn Transport>) -> Self {
        Self(Arc::downgrade(&tranport))
    }

    pub fn load_config<T>(&self, ty: DeviceConfigTy) -> T
    where
        T: Sized + TryFrom<u64>,
        <T as TryFrom<u64>>::Error: std::fmt::Debug,
    {
        let transport = self.0.upgrade().unwrap();

        let size = core::mem::size_of::<T>()
            .try_into()
            .expect("load_config: invalid size");

        let value = transport.load_config(ty as u8, size);
        T::try_from(value).unwrap()
    }

    /// Returns the capacity of the block device in bytes.
    #[inline]
    pub fn capacity(&self) -> u64 {
        self.load_config(DeviceConfigTy::Capacity)
    }

    #[inline]
    pub fn block_size(&self) -> u32 {
        self.load_config(DeviceConfigTy::BlkSize)
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

fn daemon(daemon: redox_daemon::Daemon) -> anyhow::Result<()> {
    let mut pcid_handle = PciFunctionHandle::connect_default();

    // Double check that we have the right device.
    //
    // 0x1001 - virtio-blk
    let pci_config = pcid_handle.config();

    assert_eq!(pci_config.func.full_device_id.device_id, 0x1001);
    log::info!("virtio-blk: initiating startup sequence :^)");

    let device = virtio_core::probe_device(&mut pcid_handle)?;
    device.transport.finalize_features();

    let queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR, &device.irq_handle)?;

    let device_space = BlockDeviceConfig::new(&device.transport);

    // At this point the device is alive!
    device.transport.run_device();

    log::info!(
        "virtio-blk: disk size: {} sectors and block size of {} bytes",
        device_space.capacity(),
        device_space.block_size()
    );

    let mut name = pci_config.func.name();
    name.push_str("_virtio_blk");

    let scheme_name = format!("disk.{}", name);

    let event_queue = event::EventQueue::new().unwrap();

    event::user_data! {
        enum Event {
            Scheme,
        }
    };

    let mut scheme = DiskScheme::new(
        Some(daemon),
        scheme_name,
        BTreeMap::from([(0, VirtioDisk::new(queue, device_space))]),
        &driver_block::FuturesExecutor,
    );

    libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    event_queue
        .subscribe(
            scheme.event_handle().raw(),
            Event::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    for event in event_queue {
        match event.unwrap().user_data {
            Event::Scheme => futures::executor::block_on(scheme.tick()).unwrap(),
        }
    }

    Ok(())
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    daemon(redox_daemon).unwrap();
    unreachable!();
}
