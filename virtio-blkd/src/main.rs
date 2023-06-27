#![deny(trivial_numeric_casts, unused_allocation)]
#![feature(int_roundings)]

use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use static_assertions::const_assert_eq;

use pcid_interface::*;
use virtio_core::spec::*;

use event::EventQueue;
use syscall::{Packet, SchemeBlockMut};

use virtio_core::utils::VolatileCell;

mod scheme;

use thiserror::Error;

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
    #[cfg(target_os = "redox")]
    virtio_core::utils::setup_logging(log::LevelFilter::Trace, "virtio-blkd");
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
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

    // Double check that we have the right device.
    //
    // 0x1001 - virtio-blk
    let pci_config = pcid_handle.fetch_config()?;

    assert_eq!(pci_config.func.devid, 0x1001);
    log::info!("virtio-blk: initiating startup sequence :^)");

    let mut device = virtio_core::probe_device(&mut pcid_handle)?;
    device.transport.finalize_features();

    let queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR)?;
    let queue_copy = queue.clone();

    let device_space = unsafe { &mut *(device.device_space as *mut BlockDeviceConfig) };

    std::thread::spawn(move || {
        let mut event_queue = EventQueue::<usize>::new().unwrap();
        let mut progress_head = 0;

        event_queue
            .add(
                device.irq_handle.as_raw_fd(),
                move |_| -> Result<Option<usize>, io::Error> {
                    // Read from ISR to acknowledge the interrupt.
                    let _isr = device.isr.get() as usize;

                    let mut inner = queue_copy.inner.lock().unwrap();
                    let used_head = inner.used.head_index();

                    if progress_head == used_head {
                        return Ok(None);
                    }

                    for i in progress_head..used_head {
                        let used = inner.used.get_element_at(i as usize);
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
                    }

                    progress_head = used_head;
                    drop(inner);

                    let mut buf = [0u8; 8];
                    device.irq_handle.read(&mut buf)?;
                    // Acknowledge the interrupt.
                    // irq_handle.write(&buf)?;
                    Ok(None)
                },
            )
            .unwrap();

        loop {
            event_queue.run().unwrap();
        }
    });

    // At this point the device is alive!
    device.transport.run_device();

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

    let mut scheme = scheme::DiskScheme::new(queue, device_space);

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
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}
