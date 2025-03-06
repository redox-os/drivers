use std::sync::Arc;

use common::{dma::Dma, sgl};
use driver_graphics::{Framebuffer, GraphicsAdapter, GraphicsScheme};
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
}

impl GraphicsAdapter for VirtGpuAdapter<'_> {
    type Framebuffer = VirtGpuFramebuffer;

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

    fn update_plane(
        &mut self,
        display_id: usize,
        framebuffer: &Self::Framebuffer,
        damage: &[Damage],
    ) {
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

            for damage in damage {
                let flush = ResourceFlush::new(
                    framebuffer.id,
                    damage.clip(framebuffer.width, framebuffer.height).into(),
                );
                let header = self.send_request(Dma::new(flush).unwrap()).await.unwrap();
                assert_eq!(header.ty, CommandTy::RespOkNodata);
            }
        });
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
