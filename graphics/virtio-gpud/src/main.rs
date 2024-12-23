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
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, Ordering};

use event::{user_data, EventQueue};
use libredox::errno::EAGAIN;
use pcid_interface::PciFunctionHandle;

use redox_scheme::{RequestKind, SignalBehavior, Socket, V2};
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

static RESOURCE_ALLOC: AtomicU32 = AtomicU32::new(1); // XXX: 0 is reserved for whatever that takes `resource_id`.

#[derive(Debug, Copy, Clone)]
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

    // FIXME we can likely use regular loads and stores as the ring buffer should provide the
    // necessary synchronization.
    resource_id: VolatileCell<ResourceId>,
    format: VolatileCell<ResourceFormat>,
    width: VolatileCell<u32>,
    height: VolatileCell<u32>,
}

impl ResourceCreate2d {
    make_getter_setter!(resource_id: ResourceId, format: ResourceFormat, width: u32, height: u32);
}

impl Default for ResourceCreate2d {
    fn default() -> Self {
        Self {
            header: ControlHeader {
                ty: VolatileCell::new(CommandTy::ResourceCreate2d),
                ..Default::default()
            },

            resource_id: VolatileCell::new(ResourceId(0)),
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
    pub fn new(resource_id: ResourceId, rect: GpuRect) -> Self {
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

    let socket: Socket<V2> = Socket::nonblock("display.virtio-gpu")?;
    let mut scheme = futures::executor::block_on(scheme::Scheme::new(
        config,
        control_queue.clone(),
        cursor_queue.clone(),
        device.transport.clone(),
    ))?;

    user_data! {
        enum Source {
            Input,
            Scheme,
        }
    }

    let event_queue: EventQueue<Source> =
        EventQueue::new().expect("virtio-gpud: failed to create event queue");
    event_queue
        .subscribe(
            scheme.inputd_handle.inner().as_raw_fd() as usize,
            Source::Input,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            socket.inner().raw(),
            Source::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    //let mut inputd_control_handle = inputd::ControlHandle::new().unwrap();
    //inputd_control_handle.activate_vt(3).unwrap();

    let all = [Source::Input, Source::Scheme];
    for event in all
        .into_iter()
        .chain(event_queue.map(|e| e.expect("virtio-gpud: failed to get next event").user_data))
    {
        match event {
            Source::Input => {
                while let Some(vt_event) = scheme
                    .inputd_handle
                    .read_vt_event()
                    .expect("virtio-gpud: failed to read display handle")
                {
                    scheme.handle_vt_event(vt_event);
                }
            }
            Source::Scheme => {
                loop {
                    let request = match socket.next_request(SignalBehavior::Restart) {
                        Ok(Some(request)) => request,
                        Ok(None) => {
                            // Scheme likely got unmounted
                            std::process::exit(0);
                        }
                        Err(err) if err.errno == EAGAIN => break,
                        Err(err) => return Err(err.into()),
                    };

                    match request.kind() {
                        RequestKind::Call(call_request) => {
                            socket
                                .write_response(
                                    call_request.handle_scheme_mut(&mut scheme),
                                    SignalBehavior::Restart,
                                )
                                .expect("virtio-gpud: failed to write display scheme");
                        }
                        RequestKind::Cancellation(_cancellation_request) => {}
                        RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                            unreachable!()
                        }
                    }
                }
            }
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
        "misc",
        "pcie",
        "virtio-gpud",
        log::LevelFilter::Trace,
        log::LevelFilter::Trace,
    );
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
