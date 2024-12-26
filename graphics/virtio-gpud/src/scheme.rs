use std::collections::{BTreeMap, HashMap};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{dma::Dma, sgl};
use inputd::{Damage, VtEvent, VtEventKind};

use redox_scheme::Scheme;
use syscall::{Error as SysError, MapFlags, EBADF, EINVAL, PAGE_SIZE};

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue, Transport};
use virtio_core::utils::VolatileCell;

use crate::*;

impl Into<GpuRect> for Damage {
    fn into(self) -> GpuRect {
        GpuRect {
            x: self.x as u32,
            y: self.y as u32,
            width: self.width as u32,
            height: self.height as u32,
        }
    }
}

struct GpuResource {
    id: ResourceId,
    sgl: sgl::Sgl,
}

#[derive(Debug, Copy, Clone)]
pub struct Display {
    width: u32,
    height: u32,
}

impl GpuScheme<'_> {
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

    async fn res_for_screen(
        &mut self,
        vt: usize,
        screen: usize,
    ) -> Result<(ResourceId, *mut u8), Error> {
        if let Some(res) = self.vts.entry(vt).or_default().get(&screen) {
            return Ok((res.id, res.sgl.as_ptr()));
        }

        let bpp = 32;
        let fb_size =
            self.displays[screen].width as usize * self.displays[screen].height as usize * bpp / 8;
        let sgl = sgl::Sgl::new(fb_size)?;

        unsafe {
            core::ptr::write_bytes(sgl.as_ptr() as *mut u8, 255, fb_size);
        }

        let res_id = ResourceId::alloc();

        // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
        let mut request = Dma::new(ResourceCreate2d::default())?;

        request.set_width(self.displays[screen].width);
        request.set_height(self.displays[screen].height);
        request.set_format(ResourceFormat::Bgrx);
        request.set_resource_id(res_id);

        let header = self.send_request(request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        // Use the allocated framebuffer from tthe guest ram, and attach it as backing
        // storage to the resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`.

        let mut mem_entries = unsafe { Dma::zeroed_slice(sgl.chunks().len())?.assume_init() };
        for (entry, chunk) in mem_entries.iter_mut().zip(sgl.chunks().iter()) {
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

        let res = self
            .vts
            .entry(vt)
            .or_default()
            .entry(screen)
            .or_insert(GpuResource { id: res_id, sgl });
        Ok((res.id, res.sgl.as_ptr()))
    }

    async fn set_scanout(&mut self, vt: usize, screen: usize) -> Result<(), Error> {
        let (res_id, _) = self.res_for_screen(vt, screen).await?;

        let scanout_request = Dma::new(SetScanout::new(
            screen as u32,
            res_id,
            GpuRect::new(
                0,
                0,
                self.displays[screen].width,
                self.displays[screen].height,
            ),
        ))?;
        let header = self.send_request(scanout_request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        self.flush(vt, screen, None).await?;

        Ok(())
    }

    /// If `damage` is `None`, the entire screen is flushed.
    async fn flush(
        &mut self,
        vt: usize,
        screen: usize,
        damage: Option<&[Damage]>,
    ) -> Result<(), Error> {
        let (res_id, _) = self.res_for_screen(vt, screen).await?;

        let req = Dma::new(XferToHost2d::new(
            res_id,
            GpuRect {
                x: 0,
                y: 0,
                width: self.displays[screen].width,
                height: self.displays[screen].height,
            },
            0,
        ))?;
        let header = self.send_request(req).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        if let Some(damage) = damage {
            for damage in damage {
                self.flush_resource(ResourceFlush::new(
                    res_id,
                    damage
                        .clip(
                            self.displays[screen].width as i32,
                            self.displays[screen].height as i32,
                        )
                        .into(),
                ))
                .await?;
            }
        } else {
            self.flush_resource(ResourceFlush::new(
                res_id,
                GpuRect {
                    x: 0,
                    y: 0,
                    width: self.displays[screen].width,
                    height: self.displays[screen].height,
                },
            ))
            .await?;
        }
        Ok(())
    }
}

struct Handle {
    screen: usize,
    vt: usize,
}

pub struct GpuScheme<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,
    transport: Arc<dyn Transport>,
    pub inputd_handle: inputd::DisplayHandle,

    handles: BTreeMap<usize /* file descriptor */, Handle>,
    /// Counter used for file descriptor allocation.
    next_id: AtomicUsize,

    active_vt: usize,
    vts: HashMap<usize, HashMap<usize, GpuResource>>,
    displays: Vec<Display>,
}

impl<'a> GpuScheme<'a> {
    pub async fn new(
        config: &'a mut GpuConfig,
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        transport: Arc<dyn Transport>,
    ) -> Result<GpuScheme<'a>, Error> {
        let displays = Self::probe(control_queue.clone(), config).await?;

        let inputd_handle = inputd::DisplayHandle::new("virtio-gpu").unwrap();

        Ok(Self {
            control_queue,
            cursor_queue,
            transport,
            inputd_handle,

            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),

            active_vt: 0,
            vts: HashMap::new(),
            displays,
        })
    }

    async fn probe(
        control_queue: Arc<Queue<'a>>,
        config: &GpuConfig,
    ) -> Result<Vec<Display>, Error> {
        let mut display_info = Self::get_display_info(control_queue.clone()).await?;
        let displays = &mut display_info.display_info[..config.num_scanouts() as usize];

        let mut result = vec![];

        for info in displays.iter() {
            log::info!(
                "virtio-gpu: opening display ({}x{}px)",
                info.rect().width,
                info.rect().height
            );

            result.push(Display {
                width: info.rect().width,
                height: info.rect().height,
            });
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

                for id in 0..self.displays.len() {
                    log::warn!("virtio-gpu: activating");

                    futures::executor::block_on(self.set_scanout(vt_event.vt, id)).unwrap();
                }

                self.active_vt = vt_event.vt;
            }

            VtEventKind::Deactivate => {
                log::info!("deactivate {}", vt_event.vt);
                // nothing to do :)
            }

            VtEventKind::Resize => {
                log::warn!("virtio-gpu: resize is not implemented yet")
            }
        }
    }
}

impl<'a> Scheme for GpuScheme<'a> {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> syscall::Result<usize> {
        if path.is_empty() {
            return Err(SysError::new(EINVAL));
        }

        let mut parts = path.split('/');
        let mut screen = parts.next().unwrap_or("").split('.');

        let vt = screen.next().unwrap_or("").parse::<usize>().unwrap();
        let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

        dbg!(vt, id);

        if id >= self.displays.len() {
            return Err(SysError::new(EINVAL));
        };

        let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.handles.insert(fd, Handle { screen: id, vt });
        Ok(fd)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).unwrap();
        let path = format!(
            "display.virtio-gpu:3.0/{}/{}",
            self.displays[handle.screen].width, self.displays[handle.screen].height
        );
        buf[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).ok_or(SysError::new(EBADF))?;
        if handle.vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the resource on the next scanout anyway
            return Ok(0);
        }
        futures::executor::block_on(self.flush(handle.vt, handle.screen, None)).unwrap();
        Ok(0)
    }

    fn read(
        &mut self,
        id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        let _handle = self.handles.get(&id).ok_or(SysError::new(EBADF))?;
        Err(SysError::new(EINVAL))
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> syscall::Result<usize> {
        let handle = self.handles.get(&id).ok_or(SysError::new(EBADF))?;

        if handle.vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the resource on the next scanout anyway
            return Ok(buf.len());
        }

        let damage = unsafe {
            core::slice::from_raw_parts(
                buf.as_ptr() as *const Damage,
                buf.len() / core::mem::size_of::<Damage>(),
            )
        };

        futures::executor::block_on(self.flush(handle.vt, handle.screen, Some(damage))).unwrap();

        Ok(buf.len())
    }

    fn close(&mut self, id: usize) -> syscall::Result<usize> {
        self.handles.remove(&id).ok_or(SysError::new(EBADF))?;
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
        let (_, ptr) =
            futures::executor::block_on(self.res_for_screen(handle.vt, handle.screen)).unwrap();
        Ok(unsafe { ptr.offset(offset as isize) } as usize)
    }
}
