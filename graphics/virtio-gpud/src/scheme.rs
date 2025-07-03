use std::sync::Arc;

use common::{dma::Dma, sgl};
use driver_graphics::{
    CursorFramebuffer, CursorPlane, Framebuffer, GraphicsAdapter, GraphicsScheme,
};
use graphics_ipc::v1::Damage;
use inputd::DisplayHandle;

use syscall::PAGE_SIZE;

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue, Transport};

use crate::*;

impl Into<GpuRect> for Damage {
    fn into(self) -> GpuRect {
        GpuRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

pub struct VirtGpuFramebuffer<'a> {
    queue: Arc<Queue<'a>>,
    id: ResourceId,
    sgl: sgl::Sgl,
    width: u32,
    height: u32,
}

impl Framebuffer for VirtGpuFramebuffer<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}

impl Drop for VirtGpuFramebuffer<'_> {
    fn drop(&mut self) {
        futures::executor::block_on(async {
            let request = Dma::new(ResourceUnref::new(self.id)).unwrap();

            let header = Dma::new(ControlHeader::default()).unwrap();
            let command = ChainBuilder::new()
                .chain(Buffer::new(&request))
                .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
                .build();

            self.queue.send(command).await;
        });
    }
}

pub struct VirtGpuCursor {
    resource_id: ResourceId,
    sgl: sgl::Sgl,
}

impl CursorFramebuffer for VirtGpuCursor {}

#[derive(Debug, Copy, Clone)]
pub struct Display {
    width: u32,
    height: u32,
    active_resource: Option<ResourceId>,
}

pub struct VirtGpuAdapter<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,
    transport: Arc<dyn Transport>,
    displays: Vec<Display>,
}

impl VirtGpuAdapter<'_> {
    pub async fn update_displays(&mut self, config: &mut GpuConfig) -> Result<(), Error> {
        let mut display_info = self.get_display_info().await?;
        let raw_displays = &mut display_info.display_info[..config.num_scanouts() as usize];

        self.displays.resize(
            raw_displays.len(),
            Display {
                width: 0,
                height: 0,
                active_resource: None,
            },
        );
        for (i, info) in raw_displays.iter().enumerate() {
            log::info!(
                "virtio-gpu: display {i} ({}x{}px)",
                info.rect.width,
                info.rect.height
            );

            if info.rect.width == 0 || info.rect.height == 0 {
                // QEMU gives all displays other than the first a zero width and height, but trying
                // to attach a zero sized framebuffer to the display will result an error, so
                // default to 640x480px.
                self.displays[i].width = 640;
                self.displays[i].height = 480;
            } else {
                self.displays[i].width = info.rect.width;
                self.displays[i].height = info.rect.height;
            }
        }

        Ok(())
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

    async fn send_request_fenced<T>(&self, request: Dma<T>) -> Result<Dma<ControlHeader>, Error> {
        let mut header = Dma::new(ControlHeader::default())?;
        header.flags |= VIRTIO_GPU_FLAG_FENCE;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&request))
            .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        Ok(header)
    }

    async fn get_display_info(&self) -> Result<Dma<GetDisplayInfo>, Error> {
        let header = Dma::new(ControlHeader::with_ty(CommandTy::GetDisplayInfo))?;

        let response = Dma::new(GetDisplayInfo::default())?;
        let command = ChainBuilder::new()
            .chain(Buffer::new(&header))
            .chain(Buffer::new(&response).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.control_queue.send(command).await;
        assert!(response.header.ty == CommandTy::RespOkDisplayInfo);

        Ok(response)
    }

    fn update_cursor(&mut self, cursor: &VirtGpuCursor, x: i32, y: i32, hot_x: i32, hot_y: i32) {
        //Transfering cursor resource to host
        futures::executor::block_on(async {
            let transfer_request = Dma::new(XferToHost2d::new(
                cursor.resource_id,
                GpuRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                },
                0,
            ))
            .unwrap();
            let header = self.send_request_fenced(transfer_request).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);
        });

        //Update the cursor position
        let request = Dma::new(UpdateCursor::update_cursor(
            x,
            y,
            hot_x,
            hot_y,
            cursor.resource_id,
        ))
        .unwrap();
        futures::executor::block_on(async {
            let command = ChainBuilder::new().chain(Buffer::new(&request)).build();
            self.cursor_queue.send(command).await;
        });
    }

    fn move_cursor(&mut self, x: i32, y: i32) {
        let request = Dma::new(MoveCursor::move_cursor(x, y)).unwrap();

        futures::executor::block_on(async {
            let command = ChainBuilder::new().chain(Buffer::new(&request)).build();
            self.cursor_queue.send(command).await;
        });
    }
}

impl<'a> GraphicsAdapter for VirtGpuAdapter<'a> {
    type Framebuffer = VirtGpuFramebuffer<'a>;
    type Cursor = VirtGpuCursor;

    fn display_count(&self) -> usize {
        self.displays.len()
    }

    fn display_size(&self, display_id: usize) -> (u32, u32) {
        (
            self.displays[display_id].width,
            self.displays[display_id].height,
        )
    }

    fn create_dumb_framebuffer(&mut self, width: u32, height: u32) -> Self::Framebuffer {
        futures::executor::block_on(async {
            let bpp = 32;
            let fb_size = width as usize * height as usize * bpp / 8;
            let sgl = sgl::Sgl::new(fb_size).unwrap();

            unsafe {
                core::ptr::write_bytes(sgl.as_ptr() as *mut u8, 255, fb_size);
            }

            let res_id = ResourceId::alloc();

            // Create a host resource using `VIRTIO_GPU_CMD_RESOURCE_CREATE_2D`.
            let request = Dma::new(ResourceCreate2d::new(
                res_id,
                ResourceFormat::Bgrx,
                width,
                height,
            ))
            .unwrap();

            let header = self.send_request(request).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            // Use the allocated framebuffer from the guest ram, and attach it as backing
            // storage to the resource just created, using `VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING`.

            let mut mem_entries =
                unsafe { Dma::zeroed_slice(sgl.chunks().len()).unwrap().assume_init() };
            for (entry, chunk) in mem_entries.iter_mut().zip(sgl.chunks().iter()) {
                *entry = MemEntry {
                    address: chunk.phys as u64,
                    length: chunk.length.next_multiple_of(PAGE_SIZE) as u32,
                    padding: 0,
                };
            }

            let attach_request =
                Dma::new(AttachBacking::new(res_id, mem_entries.len() as u32)).unwrap();
            let header = Dma::new(ControlHeader::default()).unwrap();
            let command = ChainBuilder::new()
                .chain(Buffer::new(&attach_request))
                .chain(Buffer::new_unsized(&mem_entries))
                .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
                .build();

            self.control_queue.send(command).await;
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            VirtGpuFramebuffer {
                queue: self.control_queue.clone(),
                id: res_id,
                sgl,
                width,
                height,
            }
        })
    }

    fn map_dumb_framebuffer(&mut self, framebuffer: &Self::Framebuffer) -> *mut u8 {
        framebuffer.sgl.as_ptr()
    }

    fn update_plane(&mut self, display_id: usize, framebuffer: &Self::Framebuffer, damage: Damage) {
        futures::executor::block_on(async {
            let req = Dma::new(XferToHost2d::new(
                framebuffer.id,
                GpuRect {
                    x: 0,
                    y: 0,
                    width: framebuffer.width,
                    height: framebuffer.height,
                },
                0,
            ))
            .unwrap();
            let header = self.send_request(req).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            // FIXME once we support resizing we also need to check that the current and target size match
            if self.displays[display_id].active_resource != Some(framebuffer.id) {
                let scanout_request = Dma::new(SetScanout::new(
                    display_id as u32,
                    framebuffer.id,
                    GpuRect::new(0, 0, framebuffer.width, framebuffer.height),
                ))
                .unwrap();
                let header = self.send_request(scanout_request).await.unwrap();
                assert_eq!(header.ty, CommandTy::RespOkNodata);
                self.displays[display_id].active_resource = Some(framebuffer.id);
            }

            let flush = ResourceFlush::new(
                framebuffer.id,
                damage.clip(framebuffer.width, framebuffer.height).into(),
            );
            let header = self.send_request(Dma::new(flush).unwrap()).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);
        });
    }

    fn supports_hw_cursor(&self) -> bool {
        true
    }

    fn create_cursor_framebuffer(&mut self) -> VirtGpuCursor {
        //Creating a new resource for the cursor
        let fb_size = 64 * 64 * 4;
        let sgl = sgl::Sgl::new(fb_size).unwrap();
        let res_id = ResourceId::alloc();

        futures::executor::block_on(async {
            unsafe {
                core::ptr::write_bytes(sgl.as_ptr() as *mut u8, 0, fb_size);
            }

            let resource_request =
                Dma::new(ResourceCreate2d::new(res_id, ResourceFormat::Bgrx, 64, 64)).unwrap();

            let header = self.send_request_fenced(resource_request).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            //Attaching cursor resource as backing storage
            let mut mem_entries =
                unsafe { Dma::zeroed_slice(sgl.chunks().len()).unwrap().assume_init() };
            for (entry, chunk) in mem_entries.iter_mut().zip(sgl.chunks().iter()) {
                *entry = MemEntry {
                    address: chunk.phys as u64,
                    length: chunk.length.next_multiple_of(PAGE_SIZE) as u32,
                    padding: 0,
                };
            }

            let attach_request =
                Dma::new(AttachBacking::new(res_id, mem_entries.len() as u32)).unwrap();
            let mut header = Dma::new(ControlHeader::default()).unwrap();
            header.flags |= VIRTIO_GPU_FLAG_FENCE;
            let command = ChainBuilder::new()
                .chain(Buffer::new(&attach_request))
                .chain(Buffer::new_unsized(&mem_entries))
                .chain(Buffer::new(&header).flags(DescriptorFlags::WRITE_ONLY))
                .build();

            self.control_queue.send(command).await;
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            //Transfering cursor resource to host
            let transfer_request = Dma::new(XferToHost2d::new(
                res_id,
                GpuRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                },
                0,
            ))
            .unwrap();
            let header = self.send_request_fenced(transfer_request).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);
        });

        VirtGpuCursor {
            resource_id: res_id,
            sgl,
        }
    }

    fn map_cursor_framebuffer(&mut self, cursor: &Self::Cursor) -> *mut u8 {
        cursor.sgl.as_ptr()
    }

    fn handle_cursor(&mut self, cursor: &CursorPlane<VirtGpuCursor>, dirty_fb: bool) {
        if dirty_fb {
            self.update_cursor(
                &cursor.framebuffer,
                cursor.x,
                cursor.y,
                cursor.hot_x,
                cursor.hot_y,
            );
        } else {
            self.move_cursor(cursor.x, cursor.y);
        }
    }
}

pub struct GpuScheme {}

impl<'a> GpuScheme {
    pub async fn new(
        config: &mut GpuConfig,
        control_queue: Arc<Queue<'a>>,
        cursor_queue: Arc<Queue<'a>>,
        transport: Arc<dyn Transport>,
    ) -> Result<(GraphicsScheme<VirtGpuAdapter<'a>>, DisplayHandle), Error> {
        let mut adapter = VirtGpuAdapter {
            control_queue,
            cursor_queue,
            transport,
            displays: vec![],
        };

        adapter.update_displays(config).await?;

        let scheme = GraphicsScheme::new(adapter, "display.virtio-gpu".to_owned());
        let handle = DisplayHandle::new("virtio-gpu").unwrap();
        Ok((scheme, handle))
    }
}
