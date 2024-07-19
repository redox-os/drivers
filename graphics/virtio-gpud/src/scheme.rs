use std::cell::OnceCell;
use std::collections::BTreeMap;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use common::{dma::Dma, sgl};
use inputd::Damage;

use syscall::{Error as SysError, MapFlags, SchemeMut, EAGAIN, EINVAL, PAGE_SIZE};

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue, Transport};
use virtio_core::utils::VolatileCell;

use crate::*;

static RESOURCE_ALLOC: AtomicU32 = AtomicU32::new(1); // XXX: 0 is reserved for whatever that takes `resource_id`.

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

    mapped: OnceCell<sgl::Sgl>,

    width: u32,
    height: u32,

    resource_id: u32,
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

            mapped: OnceCell::new(),

            width: 1920,
            height: 1080,
            transport,

            id,
            resource_id: RESOURCE_ALLOC.fetch_add(1, Ordering::SeqCst),

            is_reseted: AtomicBool::new(false),
        }
    }

    async fn init(&self) -> Result<(), Error> {
        if !self.is_reseted.load(Ordering::SeqCst) {
            // The device is already initialized.
            return Ok(());
        }

        self.is_reseted.store(false, Ordering::SeqCst);

        log::info!("virtio-gpu: initializing GPU after a reset");

        crate::reinit(self.control_queue.clone(), self.cursor_queue.clone())?;
        self.remap_screen().await?;

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

    // TODO: Is this a no-op?
    async fn remap_screen(&self) -> Result<*mut u8, Error> {
        let bpp = 32;

        let fb_size = self.width as usize * self.height as usize * bpp / 8;

        let mapped = self.mapped.get().unwrap();
        self.map_screen_with(0, fb_size, mapped.as_ptr(), mapped.chunks())
            .await
    }

    async fn map_screen(&self, offset: usize) -> Result<*mut u8, Error> {
        if let Some(mapped) = self.mapped.get() {
            return Ok(mapped.as_ptr().wrapping_add(offset));
        }

        let bpp = 32;
        let fb_size = self.width as usize * self.height as usize * bpp / 8;
        let mapped = sgl::Sgl::new(fb_size)?;

        unsafe {
            core::ptr::write_bytes(mapped.as_ptr() as *mut u8, 255, fb_size);
        }
        let _ = self.mapped.set(mapped);
        let mapped = self.mapped.get().unwrap();

        self.map_screen_with(offset, fb_size, mapped.as_ptr(), mapped.chunks())
            .await
    }

    async fn map_screen_with(
        &self,
        offset: usize,
        _size: usize,
        virt: *mut u8,
        chunks: &[sgl::Chunk],
    ) -> Result<*mut u8, Error> {
        // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
        let mut request = Dma::new(ResourceCreate2d::default())?;

        request.set_width(self.width);
        request.set_height(self.height);
        request.set_format(ResourceFormat::Bgrx);
        request.set_resource_id(self.resource_id);

        self.send_request(request).await?;

        // Use the allocated framebuffer from tthe guest ram, and attach it as backing
        // storage to the resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`.

        let mut mem_entries = unsafe { Dma::zeroed_slice(chunks.len())?.assume_init() };
        for (entry, chunk) in mem_entries.iter_mut().zip(chunks.iter()) {
            *entry = MemEntry {
                address: chunk.phys as u64,
                length: chunk.length.next_multiple_of(PAGE_SIZE) as u32,
                padding: 0,
            };
        }

        let attach_request = Dma::new(AttachBacking::new(
            self.resource_id,
            mem_entries.len() as u32,
        ))?;
        let header = Dma::new(ControlHeader::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&attach_request))
            .chain(Buffer::new_unsized(&mem_entries))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let scanout_request = Dma::new(SetScanout::new(
            self.id as u32,
            self.resource_id,
            GpuRect::new(0, 0, self.width, self.height),
        ))?;
        let header = self.send_request(scanout_request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        self.flush(None).await?;

        Ok(virt.wrapping_add(offset))
    }

    /// If `damage` is `None`, the entire screen is flushed.
    async fn flush(&self, damage: Option<&Damage>) -> Result<(), Error> {
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
            self.resource_id,
            GpuRect {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            },
        ))?;
        let header = self.send_request(req).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        self.flush_resource(ResourceFlush::new(self.resource_id, damage.clone()))
            .await?;
        Ok(())
    }

    /// This detaches any backing pages from the display and unrefs the resource. Also resets the
    /// device, which is required to go back to legacy mode.
    async fn detach(&self) -> Result<(), Error> {
        let request = Dma::new(DetachBacking::new(self.resource_id))?;
        let header = self.send_request(request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let request = Dma::new(ResourceUnref::new(self.resource_id))?;
        let header = self.send_request(request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        // Go back to legacy mode.
        self.transport.reset();
        self.is_reseted.store(true, Ordering::SeqCst);

        Ok(())
    }
}

enum Handle<'a> {
    Vt {
        display: Arc<Display<'a>>,
        vt: usize,
    },
    Input,
}

pub struct Scheme<'a> {
    handles: BTreeMap<usize /* file descriptor */, Handle<'a>>,
    /// Counter used for file descriptor allocation.
    next_id: AtomicUsize,
    displays: Vec<Arc<Display<'a>>>,
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

        Ok(Self {
            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),
            displays,
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
}

impl<'a> SchemeMut for Scheme<'a> {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        if path == "handle" {
            let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.handles.insert(fd, Handle::Input);

            return Ok(fd);
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
            Handle::Vt {
                display: display.clone(),
                vt,
            },
        );
        Ok(fd)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        match self.handles.get(&id).unwrap() {
            Handle::Vt { display, .. } => {
                let bytes_copied = futures::executor::block_on(display.get_fpath(buf)).unwrap();
                Ok(bytes_copied)
            }

            Handle::Input => unreachable!(),
        }
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        match self.handles.get(&id).ok_or(SysError::new(EINVAL))? {
            Handle::Vt { display, .. } => {
                futures::executor::block_on(display.flush(None)).unwrap();
                Ok(0)
            }

            _ => unreachable!(),
        }
    }

    fn read(&mut self, _id: usize, _buf: &mut [u8]) -> syscall::Result<usize> {
        // TODO: figure out how to get input lol
        log::warn!("virtio_gpu::read is a stub!");
        Ok(0)
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        match self.handles.get(&id).ok_or(SysError::new(EINVAL))? {
            Handle::Vt { display, .. } => {
                // The VT is not active and the device is reseted. Ask them to try
                // again later.
                if display.is_reseted.load(Ordering::SeqCst) {
                    return Err(SysError::new(EAGAIN));
                }

                let damages = unsafe {
                    core::slice::from_raw_parts(
                        buf.as_ptr() as *const Damage,
                        buf.len() / core::mem::size_of::<Damage>(),
                    )
                };

                for damage in damages {
                    futures::executor::block_on(display.flush(Some(damage))).unwrap();
                }

                Ok(buf.len())
            }

            Handle::Input => {
                use inputd::Cmd as DisplayCommand;

                let command = inputd::parse_command(buf).unwrap();

                match command {
                    DisplayCommand::Activate { vt } => {
                        let target_vt = vt;

                        for handle in self.handles.values() {
                            if let Handle::Vt { display, vt } = handle {
                                if *vt != target_vt {
                                    continue;
                                }

                                futures::executor::block_on(display.init()).unwrap();
                            }
                        }
                    }

                    DisplayCommand::Deactivate(target_vt) => {
                        for handle in self.handles.values() {
                            if let Handle::Vt { display, vt } = handle {
                                if *vt != target_vt {
                                    continue;
                                }

                                futures::executor::block_on(display.detach()).unwrap();
                                break;
                            }
                        }

                        // for display in self.displays.iter() {
                        //     futures::executor::block_on(display.detach()).unwrap();
                        // }
                    }

                    DisplayCommand::Resize { .. } => {
                        log::warn!("virtio-gpu: resize is not implemented yet")
                    }
                }

                Ok(buf.len())
            }
        }
    }

    fn seek(&mut self, _id: usize, _pos: isize, _whence: usize) -> syscall::Result<isize> {
        todo!()
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
        match self.handles.get(&id).ok_or(SysError::new(EINVAL))? {
            Handle::Vt { display, .. } => Ok(futures::executor::block_on(
                display.map_screen(offset as usize),
            )
            .unwrap() as usize),
            _ => unreachable!(),
        }
    }
}
