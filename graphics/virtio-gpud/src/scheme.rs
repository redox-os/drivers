use std::sync::Arc;

use common::{dma::Dma, sgl};
use driver_graphics::{Cursor, Framebuffer, GraphicsAdapter, GraphicsScheme};
use graphics_ipc::v1::{CursorDamage, Damage};
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

pub struct VirtGpuFramebuffer {
    id: ResourceId,
    sgl: sgl::Sgl,
    width: u32,
    height: u32,
}

impl Framebuffer for VirtGpuFramebuffer {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}

pub struct VirtGpuCursor {
    resource_id: ResourceId,
    sgl: sgl::Sgl,
    set: bool,
}

impl Cursor for VirtGpuCursor {}

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

    fn update_cursor(&mut self, cursor_damage: CursorDamage, cursor: &mut VirtGpuCursor) {
        let x = cursor_damage.x;
        let y = cursor_damage.y;
        let hot_x = cursor_damage.hot_x;
        let hot_y = cursor_damage.hot_y;

        let w: i32 = cursor_damage.width;
        let h: i32 = cursor_damage.height;
        let cursor_image = cursor_damage.cursor_img_bytes;

        //Clear previous image from backing storage
        unsafe {
            core::ptr::write_bytes(cursor.sgl.as_ptr() as *mut u8, 0, 64 * 64 * 4);
        }

        //Write image to backing storage
        for row in 0..h {
            let start: usize = (w * row) as usize;
            let end: usize = (w * row + w) as usize;

            unsafe {
                core::ptr::copy_nonoverlapping(
                    cursor_image[start..end].as_ptr(),
                    (cursor.sgl.as_ptr() as *mut u32).offset(64 * row as isize),
                    w as usize,
                );
            }
        }

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

    fn move_cursor(&self, cursor_damage: CursorDamage, cursor: &mut VirtGpuCursor) {
        let x = cursor_damage.x;
        let y = cursor_damage.y;
        let hot_x = cursor_damage.hot_x;
        let hot_y = cursor_damage.hot_y;

        let request = Dma::new(MoveCursor::move_cursor(
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
}

impl GraphicsAdapter for VirtGpuAdapter<'_> {
    type Framebuffer = VirtGpuFramebuffer;
    type Cursor = VirtGpuCursor;

    fn displays(&self) -> Vec<usize> {
        self.displays.iter().enumerate().map(|(i, _)| i).collect()
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
            sgl: sgl,
            set: false,
        }
    }

    fn handle_cursor(&mut self, cursor_damage: CursorDamage, cursor: &mut VirtGpuCursor) {
        if !cursor.set {
            cursor.set = true;
            self.update_cursor(cursor_damage, cursor);
        }

        if cursor_damage.header == 0 {
            self.move_cursor(cursor_damage, cursor);
        } else {
            self.update_cursor(cursor_damage, cursor);
        }
    }
}

pub struct GpuScheme {}

impl<'a> GpuScheme {
    pub async fn new(
        config: &'a mut GpuConfig,
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

        let mut display_info = adapter.get_display_info().await?;
        let raw_displays = &mut display_info.display_info[..config.num_scanouts() as usize];

        for info in raw_displays.iter() {
            log::info!(
                "virtio-gpu: opening display ({}x{}px)",
                info.rect.width,
                info.rect.height
            );

            if info.rect.width == 0 || info.rect.height == 0 {
                // QEMU gives all displays other than the first a zero width and height, but trying
                // to attach a zero sized framebuffer to the display will result an error, so
                // default to 640x480px.
                adapter.displays.push(Display {
                    width: 640,
                    height: 480,
                    active_resource: None,
                });
            } else {
                adapter.displays.push(Display {
                    width: info.rect.width,
                    height: info.rect.height,
                    active_resource: None,
                });
            }
        }

        let inputd_handle = DisplayHandle::new("virtio-gpu").unwrap();

        Ok((
            GraphicsScheme::new(adapter, "display.virtio-gpu".to_owned()),
            inputd_handle,
        ))
    }
}
