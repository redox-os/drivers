use std::collections::BTreeMap;

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use syscall::{Dma, Error as SysError, SchemeMut, EINVAL};

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue};
use virtio_core::utils::VolatileCell;

use crate::*;

static RESOURCE_ALLOC: AtomicU32 = AtomicU32::new(1); // XXX: 0 is reserved for whatever that takes `resource_id`.

pub struct Display<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,

    mapped: Option<usize>,

    width: u32,
    height: u32,

    resource_id: u32,
    id: usize,
}

impl<'a> Display<'a> {
    pub fn new(
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        _display_info: &mut DisplayInfo,
        id: usize,
    ) -> Self {
        Self {
            control_queue,
            cursor_queue,

            mapped: None,

            width: 1920,
            height: 1080,

            id,
            resource_id: RESOURCE_ALLOC.fetch_add(1, Ordering::SeqCst),
        }
    }

    async fn get_fpath(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        let path = format!("display/virtio-gpu:3.0/{}/{}", self.width, self.height);

        // Copy the path into the target buffer.
        buffer[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    async fn send_request<T>(&mut self, request: Dma<T>) -> Result<Dma<ControlHeader>, Error> {
        let header = Dma::new(ControlHeader::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&request))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        Ok(header)
    }

    async fn flush_resource(&mut self, flush: ResourceFlush) -> Result<(), Error> {
        let header = self.send_request(Dma::new(flush)?).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        Ok(())
    }

    async fn map_screen(&mut self, offset: usize) -> Result<usize, Error> {
        if let Some(mapped) = self.mapped {
            return Ok(mapped + offset);
        }

        // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
        let mut request = Dma::new(ResourceCreate2d::default())?;

        request.set_width(self.width);
        request.set_height(self.height);
        request.set_format(ResourceFormat::Bgrx);
        request.set_resource_id(self.resource_id);

        self.send_request(request).await?;

        // Allocate a framebuffer from guest ram, and attach it as backing storage to the
        // resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`. Scatter
        // lists are supported, so the framebuffer doesn’t need to be contignous in guest
        // physical memory.
        let bpp = 32;
        let fb_size = (self.width as usize * self.height as usize * bpp / 8)
            .next_multiple_of(syscall::PAGE_SIZE);
        let address = unsafe { syscall::physalloc(fb_size) }? as u64;
        let mapped = unsafe {
            syscall::physmap(
                address as usize,
                fb_size,
                syscall::PhysmapFlags::PHYSMAP_WRITE,
            )
        }?;

        unsafe {
            core::ptr::write_bytes(mapped as *mut u8, 255, fb_size);
        }

        let entry = Dma::new(MemEntry {
            address,
            length: fb_size as u32,
            padding: 0,
        })?;

        let attach_request = Dma::new(AttachBacking::new(self.resource_id, 1))?;
        let header = Dma::new(ControlHeader::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&attach_request))
            .chain(Buffer::new(&entry))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);
        self.flush().await?;
        self.mapped = Some(mapped);
        Ok(mapped + offset)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let rect = GpuRect::new(0, 0, self.width, self.height);
        let scanout_request = Dma::new(SetScanout::new(self.id as u32, self.resource_id, rect))?;
        let header = self.send_request(scanout_request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let rect = GpuRect::new(0, 0, self.width, self.height);
        let req = Dma::new(XferToHost2d::new(self.resource_id, rect))?;
        let header = self.send_request(req).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let rect = GpuRect::new(0, 0, self.width, self.height);
        self.flush_resource(ResourceFlush::new(self.resource_id, rect))
            .await?;
        Ok(())
    }
}

pub struct Scheme<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,
    config: &'a mut GpuConfig,
    /// File descriptor allocator.
    next_id: AtomicUsize,
    handles: BTreeMap<usize /* file descriptor */, Display<'a>>,
}

impl<'a> Scheme<'a> {
    pub fn new(
        config: &'a mut GpuConfig,
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
    ) -> Result<Self, Error> {
        Ok(Self {
            control_queue,
            cursor_queue,
            config,
            next_id: AtomicUsize::new(0),
            handles: BTreeMap::new(),
        })
    }

    async fn open_display(&self, id: usize) -> Result<Display<'a>, Error> {
        let mut display_info = self.get_display_info().await?;
        let displays = &mut display_info.display_info[..self.config.num_scanouts() as usize];

        let display = displays.get_mut(id).ok_or(SysError::new(syscall::ENOENT))?;

        log::info!(
            "virtio-gpu: opening display ({}x{}px)",
            display.rect.width(),
            display.rect.height()
        );

        Ok(Display::new(
            self.control_queue.clone(),
            self.cursor_queue.clone(),
            display,
            id,
        ))
    }

    async fn get_display_info(&self) -> Result<Dma<GetDisplayInfo>, Error> {
        let header = Dma::new(ControlHeader {
            ty: VolatileCell::new(CommandTy::GetDisplayInfo),
            ..Default::default()
        })?;

        let response = Dma::new(GetDisplayInfo::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&header))
            .chain(Buffer::new(&response).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        assert!(response.header.ty.get() == CommandTy::RespOkDisplayInfo);

        Ok(response)
    }
}

impl<'a> SchemeMut for Scheme<'a> {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        dbg!(&path);

        let mut parts = path.split('/');
        let mut screen = parts.next().unwrap_or("").split('.');

        let vt_index = screen.next().unwrap_or("").parse::<usize>().unwrap_or(1);
        let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

        dbg!(&vt_index, &id);

        let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
        let display = futures::executor::block_on(self.open_display(id))
            .map_err(|_| SysError::new(syscall::ENOENT))?;

        self.handles.insert(fd, display);
        Ok(fd)
    }

    fn dup(&mut self, _old_id: usize, _buf: &[u8]) -> syscall::Result<usize> {
        todo!()
    }

    fn fevent(
        &mut self,
        _id: usize,
        _flags: syscall::EventFlags,
    ) -> syscall::Result<syscall::EventFlags> {
        log::warn!("fevent is a stub!");
        Ok(syscall::EventFlags::empty())
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
        let bytes_copied = futures::executor::block_on(handle.get_fpath(buf)).unwrap();

        Ok(bytes_copied)
    }

    fn fmap_old(&mut self, id: usize, map: &syscall::OldMap) -> syscall::Result<usize> {
        self.fmap(
            id,
            &syscall::Map {
                offset: map.offset,
                size: map.size,
                flags: map.flags,
                address: 0,
            },
        )
    }

    fn fmap(&mut self, id: usize, map: &syscall::Map) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
        Ok(futures::executor::block_on(handle.map_screen(map.offset)).unwrap())
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
        futures::executor::block_on(handle.flush()).unwrap();
        Ok(0)
    }

    fn read(&mut self, _id: usize, _buf: &mut [u8]) -> syscall::Result<usize> {
        // TODO: figure out how to get input lol
        log::warn!("virtio_gpu::read is a stub!");
        Ok(0)
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        // SAFETY: lmao
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                handle.mapped.unwrap() as *mut u8,
                buf.len(),
            );
        }

        futures::executor::block_on(handle.flush()).unwrap();
        Ok(buf.len())
    }

    fn seek(&mut self, _id: usize, _pos: isize, _whence: usize) -> syscall::Result<isize> {
        todo!()
    }

    fn close(&mut self, _id: usize) -> syscall::Result<usize> {
        Ok(0)
    }
}
