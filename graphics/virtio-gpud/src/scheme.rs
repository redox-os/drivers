use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use common::{dma::Dma, sgl};
use inputd::{Damage, VtEvent, VtEventKind};

use redox_scheme::SchemeMut;
use syscall::{Error as SysError, MapFlags, EINVAL, PAGE_SIZE};

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue, Transport};
use virtio_core::utils::VolatileCell;

use crate::*;

impl Into<GpuRect> for &Damage {
    fn into(self) -> GpuRect {
        GpuRect {
            x: self.x as u32,
            y: self.y as u32,
            width: self.width as u32,
            height: self.height as u32,
        }
    }
}

pub struct Display<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,
    transport: Arc<dyn Transport>,

    active_vt: RefCell<usize>,
    vts_map: RefCell<HashMap<usize, sgl::Sgl>>,
    vts_res: RefCell<HashMap<usize, ResourceId>>,

    width: u32,
    height: u32,

    id: usize,

    is_reseted: AtomicBool,
}

impl<'a> Display<'a> {
    pub fn new(
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        transport: Arc<dyn Transport>,
        id: usize,
    ) -> Self {
        Self {
            control_queue,
            cursor_queue,

            active_vt: RefCell::new(0),
            vts_map: RefCell::new(HashMap::new()),
            vts_res: RefCell::new(HashMap::new()),

            width: 1920,
            height: 1080,
            transport,

            id,

            is_reseted: AtomicBool::new(false),
        }
    }

    async fn init(&self, vt: usize) -> Result<(), Error> {
        if !self.is_reseted.load(Ordering::SeqCst) {
            // The device is already initialized.
            self.set_scanout(vt).await?;
            return Ok(());
        }

        self.is_reseted.store(false, Ordering::SeqCst);

        log::info!("virtio-gpu: initializing GPU after a reset");

        crate::reinit(self.control_queue.clone(), self.cursor_queue.clone())?;
        self.set_scanout(vt).await?;

        Ok(())
    }

    async fn get_fpath(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        let path = format!("display.virtio-gpu:3.0/{}/{}", self.width, self.height);

        // Copy the path into the target buffer.
        buffer[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    async fn send_request<T>(&self, request: Dma<T>) -> Result<Dma<ControlHeader>, Error> {
        let header = Dma::new(ControlHeader::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&request))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        Ok(header)
    }

    async fn flush_resource(&self, flush: ResourceFlush) -> Result<(), Error> {
        let header = self.send_request(Dma::new(flush)?).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        Ok(())
    }

    async fn mmap_screen(&self, vt: usize, offset: usize) -> Result<*mut u8, Error> {
        if let Some(sgl) = self.vts_map.borrow().get(&vt) {
            return Ok(sgl.as_ptr().wrapping_add(offset));
        }

        let bpp = 32;
        let fb_size = self.width as usize * self.height as usize * bpp / 8;
        let mapped = sgl::Sgl::new(fb_size)?;

        unsafe {
            core::ptr::write_bytes(mapped.as_ptr() as *mut u8, 255, fb_size);
        }

        let mut mapped_vts = self.vts_map.borrow_mut();
        let sgl = mapped_vts.entry(vt).or_insert(mapped);
        Ok(sgl.as_ptr().wrapping_add(offset))
    }

    async fn create_res_for_screen(&self, vt: usize) -> Result<ResourceId, Error> {
        if let Some(&res_id) = self.vts_res.borrow().get(&vt) {
            return Ok(res_id);
        }

        self.mmap_screen(vt, 0).await?;

        let vts_map = self.vts_map.borrow();
        let mapped = &vts_map.get(&vt).unwrap();

        let res_id = ResourceId::alloc();

        // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
        let mut request = Dma::new(ResourceCreate2d::default())?;

        request.set_width(self.width);
        request.set_height(self.height);
        request.set_format(ResourceFormat::Bgrx);
        request.set_resource_id(res_id);

        let header = self.send_request(request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        // Use the allocated framebuffer from tthe guest ram, and attach it as backing
        // storage to the resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`.

        let mut mem_entries = unsafe { Dma::zeroed_slice(mapped.chunks().len())?.assume_init() };
        for (entry, chunk) in mem_entries.iter_mut().zip(mapped.chunks().iter()) {
            *entry = MemEntry {
                address: chunk.phys as u64,
                length: chunk.length.next_multiple_of(PAGE_SIZE) as u32,
                padding: 0,
            };
        }

        let attach_request = Dma::new(AttachBacking::new(res_id, mem_entries.len() as u32))?;
        let header = Dma::new(ControlHeader::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&attach_request))
            .chain(Buffer::new_unsized(&mem_entries))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let mut mapped_vts = self.vts_res.borrow_mut();
        mapped_vts.insert(vt, res_id);
        Ok(res_id)
    }

    async fn set_scanout(&self, vt: usize) -> Result<(), Error> {
        let res_id = self.create_res_for_screen(vt).await?;

        let scanout_request = Dma::new(SetScanout::new(
            self.id as u32,
            res_id,
            GpuRect::new(0, 0, self.width, self.height),
        ))?;
        let header = self.send_request(scanout_request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        self.flush(vt, None).await?;

        Ok(())
    }

    /// If `damage` is `None`, the entire screen is flushed.
    async fn flush(&self, vt: usize, damage: Option<&Damage>) -> Result<(), Error> {
        if vt != *self.active_vt.borrow() {
            return Ok(());
        }

        let damage = if let Some(damage) = damage {
            damage.into()
        } else {
            GpuRect {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            }
        };

        let req = Dma::new(XferToHost2d::new(
            self.vts_res.borrow()[&vt],
            GpuRect {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            },
        ))?;
        let header = self.send_request(req).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        self.flush_resource(ResourceFlush::new(
            self.vts_res.borrow()[&vt],
            damage.clone(),
        ))
        .await?;
        Ok(())
    }

    /// This detaches any backing pages from the display and unrefs the resource. Also resets the
    /// device, which is required to go back to legacy mode.
    async fn detach(&self) -> Result<(), Error> {
        for (_vt, res_id) in self.vts_res.borrow_mut().drain() {
            let request = Dma::new(DetachBacking::new(res_id))?;
            let header = self.send_request(request).await?;
            assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

            let request = Dma::new(ResourceUnref::new(res_id))?;
            let header = self.send_request(request).await?;
            assert_eq!(header.ty.get(), CommandTy::RespOkNodata);
        }

        // Go back to legacy mode.
        self.transport.reset();
        self.is_reseted.store(true, Ordering::SeqCst);

        Ok(())
    }
}

struct Handle<'a> {
    display: Arc<Display<'a>>,
    vt: usize,
}

pub struct Scheme<'a> {
    handles: BTreeMap<usize /* file descriptor */, Handle<'a>>,
    /// Counter used for file descriptor allocation.
    next_id: AtomicUsize,
    displays: Vec<Arc<Display<'a>>>,
    pub inputd_handle: inputd::DisplayHandle,
}

impl<'a> Scheme<'a> {
    pub async fn new(
        config: &'a mut GpuConfig,
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        transport: Arc<dyn Transport>,
    ) -> Result<Scheme<'a>, Error> {
        let displays = Self::probe(
            control_queue.clone(),
            cursor_queue.clone(),
            transport.clone(),
            config,
        )
        .await?;

        let mut inputd_handle = inputd::DisplayHandle::new("virtio-gpu").unwrap();
        // FIXME make vesad handoff control over all it's VT's instead
        inputd_handle.register_vt().unwrap();

        Ok(Self {
            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),
            displays,
            inputd_handle,
        })
    }

    async fn probe(
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        transport: Arc<dyn Transport>,
        config: &GpuConfig,
    ) -> Result<Vec<Arc<Display<'a>>>, Error> {
        let mut display_info = Self::get_display_info(control_queue.clone()).await?;
        let displays = &mut display_info.display_info[..config.num_scanouts() as usize];

        let mut result = vec![];

        for (id, info) in displays.iter().enumerate() {
            log::info!(
                "virtio-gpu: opening display ({}x{}px)",
                info.rect().width,
                info.rect().height
            );

            let display = Display::new(
                control_queue.clone(),
                cursor_queue.clone(),
                transport.clone(),
                id,
            );
            // FIXME this is a hack to avoid breaking things while we need to co-exist with vesad
            // Somehow necessary to ensure that creating a resource on the first reinitialization
            // after this detach doesn't fail.
            display.init(1).await?;
            display.detach().await?;

            result.push(Arc::new(display));
        }

        Ok(result)
    }

    async fn get_display_info(control_queue: Arc<Queue<'a>>) -> Result<Dma<GetDisplayInfo>, Error> {
        let header = Dma::new(ControlHeader {
            ty: VolatileCell::new(CommandTy::GetDisplayInfo),
            ..Default::default()
        })?;

        let response = Dma::new(GetDisplayInfo::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&header))
            .chain(Buffer::new(&response).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        control_queue.send(command).await;
        assert!(response.header.ty.get() == CommandTy::RespOkDisplayInfo);

        Ok(response)
    }

    pub fn handle_vt_event(&mut self, vt_event: VtEvent) {
        match vt_event.kind {
            VtEventKind::Activate => {
                log::info!("activate {}", vt_event.vt);

                for display in &self.displays {
                    log::warn!("virtio-gpu: activating");

                    futures::executor::block_on(display.init(vt_event.vt)).unwrap();

                    *display.active_vt.borrow_mut() = vt_event.vt;
                }
            }

            VtEventKind::Deactivate => {
                log::info!("deactivate {}", vt_event.vt);

                for handle in self.handles.values() {
                    if handle.vt != vt_event.vt {
                        continue;
                    }

                    log::warn!("virtio-gpu: deactivating");

                    futures::executor::block_on(handle.display.detach()).unwrap();
                    break;
                }

                // for display in self.displays.iter() {
                //     futures::executor::block_on(display.detach()).unwrap();
                // }
            }

            VtEventKind::Resize => {
                log::warn!("virtio-gpu: resize is not implemented yet")
            }
        }
    }
}

impl<'a> SchemeMut for Scheme<'a> {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        if path.is_empty() {
            return Err(SysError::new(EINVAL));
        }

        let mut parts = path.split('/');
        let mut screen = parts.next().unwrap_or("").split('.');

        let vt = screen.next().unwrap_or("").parse::<usize>().unwrap();
        let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

        dbg!(vt, id);

        let display = self.displays.get(id).ok_or(SysError::new(EINVAL))?;

        let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.handles.insert(
            fd,
            Handle {
                display: display.clone(),
                vt,
            },
        );
        Ok(fd)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).unwrap();
        let bytes_copied = futures::executor::block_on(handle.display.get_fpath(buf)).unwrap();
        Ok(bytes_copied)
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).ok_or(SysError::new(EINVAL))?;
        futures::executor::block_on(handle.display.flush(handle.vt, None)).unwrap();
        Ok(0)
    }

    fn read(
        &mut self,
        _id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        // TODO: figure out how to get input lol
        log::warn!("virtio_gpu::read is a stub!");
        Ok(0)
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).ok_or(SysError::new(EINVAL))?;

        // The VT is not active and the device is reseted. Ignore the damage. We will recreate the
        // backing storage from scratch next time we initialize, which is equivalent to damaging the
        // entire buffer.
        if handle.display.is_reseted.load(Ordering::SeqCst) {
            return Ok(buf.len());
        }

        let damages = unsafe {
            core::slice::from_raw_parts(
                buf.as_ptr() as *const Damage,
                buf.len() / core::mem::size_of::<Damage>(),
            )
        };

        for damage in damages {
            futures::executor::block_on(handle.display.flush(handle.vt, Some(damage))).unwrap();
        }

        Ok(buf.len())
    }

    fn close(&mut self, _id: usize) -> syscall::Result<usize> {
        Ok(0)
    }
    fn mmap_prep(
        &mut self,
        id: usize,
        offset: u64,
        size: usize,
        flags: MapFlags,
    ) -> syscall::Result<usize> {
        log::info!("KSMSG MMAP {} {:?} {} {}", id, flags, offset, size);
        let handle = self.handles.get(&id).ok_or(SysError::new(EINVAL))?;
        let ptr =
            futures::executor::block_on(handle.display.mmap_screen(handle.vt, offset as usize))
                .unwrap();
        Ok(ptr as usize)
    }
}
