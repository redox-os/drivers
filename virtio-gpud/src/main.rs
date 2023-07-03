//! `virtio-gpu` is a virtio based graphics adapter. It can operate in 2D mode and in 3D mode.
//!
//! XXX: 3D mode will offload rendering ops to the host gpu and therefore requires a GPU with 3D support
//! on the host machine.

#![feature(async_closure)]

use std::fs::File;
use std::io::{Read, Write};

use pcid_interface::PcidServerHandle;

use syscall::{Packet, SchemeMut};
use virtio_core::utils::VolatileCell;
use virtio_core::MSIX_PRIMARY_VECTOR;

mod scheme;

// const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;
const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

#[repr(C)]
pub struct GpuConfig {
    /// Signals pending events to the driver.
    pub events_read: VolatileCell<u32>, // read-only
    /// Clears pending events in the device (write-to-clear).
    pub events_clear: VolatileCell<u32>, // write-only

    pub min_scanouts: VolatileCell<u32>,
    pub num_capsets: VolatileCell<u32>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u32)]
pub enum CommandTy {
    Undefined = 0,

    // 2D commands
    GetDisplayInfo = 0x0100,
    ResourceCreate2d,
    ResourceUnref,
    SetScanout,
    ResourceFlush,
    TransferToHost2d,
    ResourceAttachBacking,
    ResourceDetachBacking,
    GetCapsetInfo,
    GetCapset,
    GetEdid,
    ResourceAssignUuid,
    ResourceCreateBlob,
    SetScanoutBlob,

    // 3D commands
    CtxCreate = 0x0200,
    CtxDestroy,
    CtxAttachResource,
    CtxDetachResource,
    ResourceCreate3d,
    TransferToHost3d,
    TransferFromHost3d,
    Submit3d,
    ResourceMapBlob,
    ResourceUnmapBlob,

    // cursor commands
    UpdateCursor = 0x0300,
    MoveCursor,

    // success responses
    RespOkNodata = 0x1100,
    RespOkDisplayInfo,
    RespOkCapsetInfo,
    RespOkCapset,
    RespOkEdid,
    RespOkResourceUuid,
    RespOkMapInfo,

    // error responses
    RespErrUnspec = 0x1200,
    RespErrOutOfMemory,
    RespErrInvalidScanoutId,
    RespErrInvalidResourceId,
    RespErrInvalidContextId,
    RespErrInvalidParameter,
}

static_assertions::const_assert_eq!(core::mem::size_of::<CommandTy>(), 4);

#[derive(Debug)]
#[repr(C)]
pub struct ControlHeader {
    pub ty: VolatileCell<CommandTy>,
    pub flags: VolatileCell<u32>,
    pub fence_id: VolatileCell<u64>,
    pub ctx_id: VolatileCell<u32>,
    pub ring_index: VolatileCell<u8>,
    padding: [u8; 3],
}

impl Default for ControlHeader {
    fn default() -> Self {
        Self {
            ty: VolatileCell::new(CommandTy::Undefined),
            flags: VolatileCell::new(0),
            fence_id: VolatileCell::new(0),
            ctx_id: VolatileCell::new(0),
            ring_index: VolatileCell::new(0),
            padding: [0; 3],
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct GpuRect {
    pub x: VolatileCell<u32>,
    pub y: VolatileCell<u32>,
    pub width: VolatileCell<u32>,
    pub height: VolatileCell<u32>,
}

#[derive(Debug)]
#[repr(C)]
pub struct DisplayInfo {
    pub rect: GpuRect,
    pub enabled: VolatileCell<u32>,
    pub flags: VolatileCell<u32>,
}

#[derive(Debug)]
#[repr(C)]
pub struct GetDisplayInfo {
    pub header: ControlHeader,
    pub display_info: [DisplayInfo; VIRTIO_GPU_MAX_SCANOUTS],
}

impl Default for GetDisplayInfo {
    fn default() -> Self {
        Self {
            header: ControlHeader {
                ty: VolatileCell::new(CommandTy::GetDisplayInfo),
                ..Default::default()
            },

            display_info: unsafe { core::mem::zeroed() },
        }
    }
}

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    let mut pcid_handle = PcidServerHandle::connect_default()?;

    // Double check that we have the right device.
    //
    // 0x1050 - virtio-gpu
    let pci_config = pcid_handle.fetch_config()?;

    assert_eq!(pci_config.func.devid, 0x1050);
    log::info!("virtio-gpu: initiating startup sequence :^)");

    let device = virtio_core::probe_device(&mut pcid_handle)?;

    // Negotiate features.
    device.transport.finalize_features();

    // Queue for sending control commands.
    let control_queue = device
        .transport
        .setup_queue(MSIX_PRIMARY_VECTOR, &device.irq_handle)?;

    // Queue for sending cursor updates.
    let cursor_queue = device
        .transport
        .setup_queue(MSIX_PRIMARY_VECTOR, &device.irq_handle)?;

    device.transport.run_device();
    deamon.ready().unwrap();

    let mut socket_file = File::create(":display/virtio-gpu")?;
    let mut scheme = scheme::Display::new(control_queue, cursor_queue);

    loop {
        let mut packet = Packet::default();
        socket_file
            .read(&mut packet)
            .expect("virtio-gpud: failed to read disk scheme");
        scheme.handle(&mut packet);
        socket_file
            .write(&packet)
            .expect("virtio-gpud: failed to read disk scheme");
    }
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

pub fn main() {
    #[cfg(target_os = "redox")]
    virtio_core::utils::setup_logging(log::LevelFilter::Trace, "virtio-gpud");
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
