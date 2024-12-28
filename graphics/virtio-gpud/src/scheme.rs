use std::sync::Arc;

use common::{dma::Dma, sgl};
use driver_graphics::{GraphicsAdapter, GraphicsScheme, Resource};
use inputd::{Damage, DisplayHandle};

use syscall::PAGE_SIZE;

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::{Error, Queue, Transport};

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

pub struct VirtGpuResource {
    id: ResourceId,
    sgl: sgl::Sgl,
    width: u32,
    height: u32,
}

impl Resource for VirtGpuResource {
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

    async fn flush_resource_inner(&self, flush: ResourceFlush) -> Result<(), Error> {
        let header = self.send_request(Dma::new(flush)?).await?;
        assert_eq!(header.ty, CommandTy::RespOkNodata);

        Ok(())
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
    type Resource = VirtGpuResource;

    fn displays(&self) -> Vec<usize> {
        self.displays.iter().enumerate().map(|(i, _)| i).collect()
    }

    fn display_size(&self, display_id: usize) -> (u32, u32) {
        (
            self.displays[display_id].width,
            self.displays[display_id].height,
        )
    }

    fn create_resource(&mut self, width: u32, height: u32) -> Self::Resource {
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

            // Use the allocated framebuffer from tthe guest ram, and attach it as backing
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

            VirtGpuResource {
                id: res_id,
                sgl,
                width,
                height,
            }
        })
    }

    fn map_resource(&mut self, resource: &Self::Resource) -> *mut u8 {
        resource.sgl.as_ptr()
    }

    fn set_scanout(&mut self, display_id: usize, resource: &Self::Resource) {
        futures::executor::block_on(async {
            let scanout_request = Dma::new(SetScanout::new(
                display_id as u32,
                resource.id,
                GpuRect::new(
                    0,
                    0,
                    self.displays[display_id].width,
                    self.displays[display_id].height,
                ),
            ))
            .unwrap();
            let header = self.send_request(scanout_request).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);
        });

        self.flush_resource(display_id, resource, None);
    }

    fn flush_resource(
        &mut self,
        _display_id: usize,
        resource: &Self::Resource,
        damage: Option<&[Damage]>,
    ) {
        futures::executor::block_on(async {
            let req = Dma::new(XferToHost2d::new(
                resource.id,
                GpuRect {
                    x: 0,
                    y: 0,
                    width: resource.width,
                    height: resource.height,
                },
                0,
            ))
            .unwrap();
            let header = self.send_request(req).await.unwrap();
            assert_eq!(header.ty, CommandTy::RespOkNodata);

            if let Some(damage) = damage {
                for damage in damage {
                    self.flush_resource_inner(ResourceFlush::new(
                        resource.id,
                        damage
                            .clip(resource.width as i32, resource.height as i32)
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
            } else {
                self.flush_resource_inner(ResourceFlush::new(
                    resource.id,
                    GpuRect {
                        x: 0,
                        y: 0,
                        width: resource.width,
                        height: resource.height,
                    },
                ))
                .await
                .unwrap();
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
                });
            } else {
                adapter.displays.push(Display {
                    width: info.rect.width,
                    height: info.rect.height,
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
