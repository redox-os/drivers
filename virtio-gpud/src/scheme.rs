use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use orbclient::Event;
use syscall::{Dma, Error as SysError, SchemeMut, EINVAL, EPERM};
use virtio_core::{
    spec::{Buffer, ChainBuilder, DescriptorFlags},
    transport::{Error, Queue},
    utils::VolatileCell,
};

use crate::*;

pub enum Handle {
    Screen { id: usize },
}

pub struct Display<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,

    display_id: usize,
    handles: BTreeMap<usize, Handle>,
    next_id: AtomicUsize,
    mapped: Option<usize>,
}

impl<'a> Display<'a> {
    pub fn new(control_queue: Arc<Queue<'a>>, cursor_queue: Arc<Queue<'a>>) -> Self {
        Self {
            control_queue,
            cursor_queue,

            display_id: 0,
            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),
            mapped: None,
        }
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

    async fn get_resolution(&self) -> Result<(u32, u32), Error> {
        let display_info = self.get_display_info().await?;

        let width = display_info.display_info[self.display_id].rect.width();
        let height = display_info.display_info[self.display_id].rect.height();

        Ok((width, height))
    }

    async fn get_fpath(&mut self, buffer: &mut [u8]) -> syscall::Result<usize> {
        let display_info = self.get_display_info().await.unwrap();

        let width = display_info.display_info[self.display_id].rect.width();
        let height = display_info.display_info[self.display_id].rect.height();

        let path = format!("display:3.0/{}/{}", width, height);

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
        // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
        let (width, height) = self.get_resolution().await?;
        let mut request = Dma::new(ResourceCreate2d::default())?;

        request.set_width(width);
        request.set_height(height);
        request.set_format(ResourceFormat::Bgrx);
        request.set_resource_id(1); // FIXME(andypython): dynamically allocate resource identifiers

        self.send_request(request).await?;

        // Allocate a framebuffer from guest ram, and attach it as backing storage to the
        // resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`. Scatter
        // lists are supported, so the framebuffer doesn’t need to be contignous in guest
        // physical memory.
        let bpp = 32;
        let fb_size =
            (width as usize * height as usize * bpp / 8).next_multiple_of(syscall::PAGE_SIZE);
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

        let attach_request = Dma::new(AttachBacking::new(1, 1))?;
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
        let (width, height) = self.get_resolution().await?;

        let rect = GpuRect::new(0, 0, width, height);
        let scanout_request = Dma::new(SetScanout::new(0, 1, rect))?;
        let header = self.send_request(scanout_request).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let rect = GpuRect::new(0, 0, width, height);
        let req = Dma::new(XferToHost2d::new(1, rect))?;
        let header = self.send_request(req).await?;
        assert_eq!(header.ty.get(), CommandTy::RespOkNodata);

        let rect = GpuRect::new(0, 0, width, height);
        self.flush_resource(ResourceFlush::new(1, rect)).await?;
        Ok(())
    }
}

impl<'a> SchemeMut for Display<'a> {
    fn open(&mut self, path: &str, _flags: usize, uid: u32, _gid: u32) -> syscall::Result<usize> {
        if path == "input" {
            if uid != 0 {
                return Err(SysError::new(EPERM));
            }

            unimplemented!("input is only supported via `display/vesa:input`")
        } else {
            let mut parts = path.split('/');
            let mut screen = parts.next().unwrap_or("").split('.');

            let vt_index = screen.next().unwrap_or("").parse::<usize>().unwrap_or(1);
            let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

            if id != self.display_id {
                return Err(SysError::new(syscall::ENOENT));
            }

            dbg!(&vt_index, &id);

            let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.handles.insert(fd, Handle::Screen { id });

            Ok(fd)
        }
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
        let bytes_copied = match handle {
            Handle::Screen { .. } => futures::executor::block_on(self.get_fpath(buf))?,
        };

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

        let a = match handle {
            Handle::Screen { .. } => {
                if let Some(mapped) = self.mapped {
                    // already mapped
                    mapped + map.offset
                } else {
                    // create the resource
                    futures::executor::block_on(self.map_screen(map.offset)).unwrap()
                }
            }
        };
        Ok(a)
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let _handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;
        futures::executor::block_on(self.flush()).unwrap();

        Ok(0)
    }

    fn read(&mut self, _id: usize, _buf: &mut [u8]) -> syscall::Result<usize> {
        // TODO: figure out how to get input lol
        log::warn!("virtio_gpu::read is a stub!");
        Ok(0)
    }

    fn write(&mut self, _id: usize, buf: &[u8]) -> syscall::Result<usize> {
        // let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        // match handle {
        //     Handle::Screen { .. } => {
        //         let size = buf.len() / core::mem::size_of::<Event>();
        //         let events =
        //             unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<Event>(), size) };

        //         dbg!(events);
        //         todo!()
        //     }
        // }

        // SAFETY: lmao
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                self.mapped.unwrap() as *mut u8,
                buf.len(),
            );
            futures::executor::block_on(self.flush()).unwrap();
        }
        Ok(buf.len())
    }

    fn seek(&mut self, _id: usize, _pos: isize, _whence: usize) -> syscall::Result<isize> {
        todo!()
    }

    fn close(&mut self, _id: usize) -> syscall::Result<usize> {
        Ok(0)
    }
}
