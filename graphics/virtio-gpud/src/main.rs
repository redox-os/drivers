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

use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, Ordering};

use event::{user_data, EventQueue};
use pcid_interface::PciFunctionHandle;

use virtio_core::utils::VolatileCell;
use virtio_core::MSIX_PRIMARY_VECTOR;

mod scheme;

const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;
const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

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

const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;
//const VIRTIO_GPU_FLAG_INFO_RING_IDX: u32 = 1 << 1;

#[derive(Debug)]
#[repr(C)]
pub struct ControlHeader {
    pub ty: CommandTy,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub ring_index: u8,
    padding: [u8; 3],
}

impl ControlHeader {
    pub fn with_ty(ty: CommandTy) -> Self {
        Self {
            ty,
            ..Default::default()
        }
    }
}

impl Default for ControlHeader {
    fn default() -> Self {
        Self {
            ty: CommandTy::Undefined,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            ring_index: 0,
            padding: [0; 3],
        }
    }
}

#[derive(Debug, Copy, Clone)]
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
    rect: GpuRect,
    pub enabled: u32,
    pub flags: u32,
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
                ty: CommandTy::GetDisplayInfo,
                ..Default::default()
            },

            display_info: unsafe { core::mem::zeroed() },
        }
    }
}

static RESOURCE_ALLOC: AtomicU32 = AtomicU32::new(1); // XXX: 0 is reserved for whatever that takes `resource_id`.

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
#[repr(C)]
pub struct ResourceId(u32);

impl ResourceId {
    fn alloc() -> Self {
        ResourceId(RESOURCE_ALLOC.fetch_add(1, Ordering::SeqCst))
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
    resource_id: ResourceId,
    format: ResourceFormat,
    width: u32,
    height: u32,
}

impl ResourceCreate2d {
    fn new(resource_id: ResourceId, format: ResourceFormat, width: u32, height: u32) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::ResourceCreate2d),
            resource_id,
            format,
            width,
            height,
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
    pub resource_id: ResourceId,
    pub num_entries: u32,
}

impl AttachBacking {
    pub fn new(resource_id: ResourceId, num_entries: u32) -> Self {
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
    pub resource_id: ResourceId,
    pub padding: u32,
}

impl DetachBacking {
    pub fn new(resource_id: ResourceId) -> Self {
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
    pub resource_id: ResourceId,
    pub padding: u32,
}

impl ResourceFlush {
    pub fn new(resource_id: ResourceId, rect: GpuRect) -> Self {
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
    pub resource_id: ResourceId,
    pub padding: u32,
}

impl ResourceUnref {
    pub fn new(resource_id: ResourceId) -> Self {
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
    pub resource_id: ResourceId,
}

impl SetScanout {
    pub fn new(scanout_id: u32, resource_id: ResourceId, rect: GpuRect) -> Self {
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
    pub resource_id: ResourceId,
    pub padding: u32,
}

impl XferToHost2d {
    pub fn new(resource_id: ResourceId, rect: GpuRect, offset: u64) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::TransferToHost2d),
            rect,
            offset,
            resource_id,
            padding: 0,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct CursorPos {
    pub scanout_id: u32,
    pub x: i32,
    pub y: i32,
    _padding: u32,
}

impl CursorPos {
    pub fn new(scanout_id: u32, x: i32, y: i32) -> Self {
        Self {
            scanout_id,
            x,
            y,
            _padding: 0,
        }
    }
}

/* VIRTIO_GPU_CMD_UPDATE_CURSOR, VIRTIO_GPU_CMD_MOVE_CURSOR */
#[derive(Debug)]
#[repr(C)]
pub struct UpdateCursor {
    pub header: ControlHeader,
    pub pos: CursorPos,
    pub resource_id: ResourceId,
    pub hot_x: i32,
    pub hot_y: i32,
    _padding: u32,
}

impl UpdateCursor {
    pub fn update_cursor(x: i32, y: i32, hot_x: i32, hot_y: i32, resource_id: ResourceId) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::UpdateCursor),
            pos: CursorPos::new(0, x, y),
            resource_id,
            hot_x,
            hot_y,
            _padding: 0,
        }
    }
}

pub struct MoveCursor {
    pub header: ControlHeader,
    pub pos: CursorPos,
    pub resource_id: ResourceId,
    pub hot_x: i32,
    pub hot_y: i32,
    _padding: u32,
}

impl MoveCursor {
    pub fn move_cursor(x: i32, y: i32) -> Self {
        Self {
            header: ControlHeader::with_ty(CommandTy::MoveCursor),
            pos: CursorPos::new(0, x, y),
            resource_id: ResourceId(0),
            hot_x: 0,
            hot_y: 0,
            _padding: 0,
        }
    }
}

static DEVICE: spin::Once<virtio_core::Device> = spin::Once::new();

fn deamon(deamon: redox_daemon::Daemon) -> anyhow::Result<()> {
    let mut pcid_handle = PciFunctionHandle::connect_default();

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

    device.transport.setup_config_notify(MSIX_PRIMARY_VECTOR);

    device.transport.run_device();
    deamon.ready().unwrap();

    let (mut scheme, mut inputd_handle) = futures::executor::block_on(scheme::GpuScheme::new(
        config,
        control_queue.clone(),
        cursor_queue.clone(),
        device.transport.clone(),
    ))?;

    user_data! {
        enum Source {
            Input,
            Scheme,
            Interrupt,
        }
    }

    let event_queue: EventQueue<Source> =
        EventQueue::new().expect("virtio-gpud: failed to create event queue");
    event_queue
        .subscribe(
            inputd_handle.inner().as_raw_fd() as usize,
            Source::Input,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            scheme.event_handle().raw(),
            Source::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            device.irq_handle.as_raw_fd() as usize,
            Source::Interrupt,
            event::EventFlags::READ,
        )
        .unwrap();

    let all = [Source::Input, Source::Scheme, Source::Interrupt];
    for event in all
        .into_iter()
        .chain(event_queue.map(|e| e.expect("virtio-gpud: failed to get next event").user_data))
    {
        match event {
            Source::Input => {
                while let Some(vt_event) = inputd_handle
                    .read_vt_event()
                    .expect("virtio-gpud: failed to read display handle")
                {
                    scheme.handle_vt_event(vt_event);
                }
            }
            Source::Scheme => {
                scheme
                    .tick()
                    .expect("virtio-gpud: failed to process scheme events");
            }
            Source::Interrupt => loop {
                let before_gen = device.transport.config_generation();

                let events = config.events_read.get();

                if events & VIRTIO_GPU_EVENT_DISPLAY != 0 {
                    futures::executor::block_on(scheme.adapter_mut().update_displays(config))
                        .unwrap();
                    scheme.notify_displays_changed();
                    config.events_clear.set(VIRTIO_GPU_EVENT_DISPLAY);
                }

                let after_gen = device.transport.config_generation();
                if before_gen == after_gen {
                    break;
                }
            },
        }
    }

    std::process::exit(0);
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

pub fn main() {
    common::setup_logging(
        "graphics",
        "pci",
        "virtio-gpud",
        common::output_level(),
        common::file_level(),
    );
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
