//! `virtio-gpu` is a virtio based graphics adapter. It can operate in 2D mode and in 3D mode.
//!
//! XXX: 3D mode will offload rendering ops to the host gpu and therefore requires a GPU with 3D support
//! on the host machine.

// Notes for the future:
//
// `virtio-gpu` 2D acceleration is just blitting. 3D acceleration has 2 kinds:
//      - virgl - OpenGL
//      - venus - Vulkan
//
// The Venus driver requires support for the following from the `virtio-gpu` kernel driver:
//     - VIRTGPU_PARAM_3D_FEATURES
//     - VIRTGPU_PARAM_CAPSET_QUERY_FIX
//     - VIRTGPU_PARAM_RESOURCE_BLOB
//     - VIRTGPU_PARAM_HOST_VISIBLE
//     - VIRTGPU_PARAM_CROSS_DEVICE
//     - VIRTGPU_PARAM_CONTEXT_INIT
//
// cc https://docs.mesa3d.org/drivers/venus.html
// cc https://docs.mesa3d.org/drivers/virgl.html

#![feature(int_roundings)]

use std::cell::UnsafeCell;
use std::fs::File;
use std::io::{Read, Write};
use std::sync::Arc;

use pcid_interface::PciFunctionHandle;

use syscall::{Packet, SchemeMut};
use virtio_core::transport::{self, Queue};
use virtio_core::utils::VolatileCell;
use virtio_core::MSIX_PRIMARY_VECTOR;

mod scheme;

// const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;
const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

macro_rules! make_getter_setter {
    ($($field:ident: $return_ty:ty),*) => {
        $(
            pub fn $field(&self) -> $return_ty {
                self.$field.get()
            }

            paste::item! {
                pub fn [<set_ $field>](&mut self, value: $return_ty) {
                    self.$field.set(value)
                }
            }
        )*
    };

    (@$field:ident: $return_ty:ty) => {
        pub fn $field(&mut self, value: $return_ty) {
            self.$field.set(value)
        }
    };
}

#[repr(C)]
pub struct GpuConfig {
    /// Signals pending events to the driver.
    pub events_read: VolatileCell<u32>, // read-only
    /// Clears pending events in the device (write-to-clear).
    pub events_clear: VolatileCell<u32>, // write-only

    pub num_scanouts: VolatileCell<u32>,
    pub num_capsets: VolatileCell<u32>,
}

impl GpuConfig {
    #[inline]
    pub fn num_scanouts(&self) -> u32 {
        self.num_scanouts.get()
    }
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

impl ControlHeader {
    pub fn with_ty(ty: CommandTy) -> Self {
        Self {
            ty: VolatileCell::new(ty),
            ..Default::default()
        }
    }
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

#[derive(Debug, Clone)]
#[repr(C)]
pub struct GpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl GpuRect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct DisplayInfo {
    rect: UnsafeCell<GpuRect>,
    pub enabled: VolatileCell<u32>,
    pub flags: VolatileCell<u32>,
}

impl DisplayInfo {
    pub fn rect(&self) -> &GpuRect {
        // SAFETY: We never give out mutable references to `self.rect`.
        unsafe { &*self.rect.get() }
    }
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

#[derive(Debug, Copy, Clone)]
#[repr(u32)]
pub enum ResourceFormat {
    Unknown = 0,

    Bgrx = 2,
    Xrgb = 4,
}

#[derive(Debug)]
#[repr(C)]
pub struct ResourceCreate2d {
    pub header: ControlHeader,

    resource_id: VolatileCell<u32>,
    format: VolatileCell<ResourceFormat>,
    width: VolatileCell<u32>,
    height: VolatileCell<u32>,
}

impl ResourceCreate2d {
    make_getter_setter!(resource_id: u32, format: ResourceFormat, width: u32, height: u32);
}

impl Default for ResourceCreate2d {
    fn default() -> Self {
        Self {
            header: ControlHeader {
                ty: VolatileCell::new(CommandTy::ResourceCreate2d),
                ..Default::default()
            },

            resource_id: VolatileCell::new(0),
            format: VolatileCell::new(ResourceFormat::Unknown),
            width: VolatileCell::new(0),
            height: VolatileCell::new(0),
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct MemEntry {
    pub address: u64,
    pub length: u32,
    pub padding: u32,
}

#[derive(Debug)]
#[repr(C)]
pub struct AttachBacking {
    pub header: ControlHeader,
    pub resource_id: u32,
    pub num_entries: u32,
}

impl AttachBacking {
    pub fn new(resource_id: u32, num_entries: u32) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::ResourceAttachBacking),
            resource_id,
            num_entries,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct DetachBacking {
    pub header: ControlHeader,
    pub resource_id: u32,
    pub padding: u32,
}

impl DetachBacking {
    pub fn new(resource_id: u32) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::ResourceDetachBacking),
            resource_id,
            padding: 0,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct ResourceFlush {
    pub header: ControlHeader,
    pub rect: GpuRect,
    pub resource_id: u32,
    pub padding: u32,
}

impl ResourceFlush {
    pub fn new(resource_id: u32, rect: GpuRect) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::ResourceFlush),
            rect,
            resource_id,
            padding: 0,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct ResourceUnref {
    pub header: ControlHeader,
    pub resource_id: u32,
    pub padding: u32,
}

impl ResourceUnref {
    pub fn new(resource_id: u32) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::ResourceUnref),
            resource_id,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct SetScanout {
    pub header: ControlHeader,
    pub rect: GpuRect,
    pub scanout_id: u32,
    pub resource_id: u32,
}

impl SetScanout {
    pub fn new(scanout_id: u32, resource_id: u32, rect: GpuRect) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::SetScanout),

            rect,
            scanout_id,
            resource_id,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct XferToHost2d {
    pub header: ControlHeader,
    pub rect: GpuRect,
    pub offset: u64,
    pub resource_id: u32,
    pub padding: u32,
}

impl XferToHost2d {
    pub fn new(resource_id: u32, rect: GpuRect) -> Self {
        Self {
            header: ControlHeader {
                ty: VolatileCell::new(CommandTy::TransferToHost2d),
                ..Default::default()
            },

            rect,
            resource_id,
            offset: 0,
            padding: 0,
        }
    }
}

static DEVICE: spin::Once<virtio_core::Device> = spin::Once::new();

fn reinit(control_queue: Arc<Queue>, cursor_queue: Arc<Queue>) -> Result<(), transport::Error> {
    let device = DEVICE.get().unwrap();

    virtio_core::reinit(device)?;
    device.transport.finalize_features();

    device.transport.reinit_queue(control_queue.clone());
    device.transport.reinit_queue(cursor_queue.clone());

    device.transport.run_device();
    Ok(())
}

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    let mut pcid_handle = PciFunctionHandle::connect_default()?;

    // Double check that we have the right device.
    //
    // 0x1050 - virtio-gpu
    let pci_config = pcid_handle.config();

    assert_eq!(pci_config.func.full_device_id.device_id, 0x1050);
    log::info!("virtio-gpu: initiating startup sequence :^)");

    let device = DEVICE.try_call_once(|| virtio_core::probe_device(&mut pcid_handle))?;
    let config = unsafe { &mut *(device.device_space as *mut GpuConfig) };

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

    let mut socket_file = File::create(":display.virtio-gpu")?;
    let mut scheme = futures::executor::block_on(scheme::Scheme::new(
        config,
        control_queue.clone(),
        cursor_queue.clone(),
        device.transport.clone(),
    ))?;

    // scheme.inputd_handle.activate(scheme.main_vt)?;

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
    common::setup_logging(
        "misc",
        "pcie",
        "virtio-gpud",
        log::LevelFilter::Trace,
        log::LevelFilter::Trace,
    );
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
