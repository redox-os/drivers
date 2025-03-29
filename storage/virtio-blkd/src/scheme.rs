use std::sync::Arc;

use common::dma::Dma;
use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::Queue;

use crate::BlockDeviceConfig;
use crate::BlockRequestTy;
use crate::BlockVirtRequest;

trait BlkExtension {
    async fn read(&self, block: u64, target: &mut [u8]) -> usize;
    async fn write(&self, block: u64, target: &[u8]) -> usize;
}

impl BlkExtension for Queue<'_> {
    async fn read(&self, block: u64, target: &mut [u8]) -> usize {
        let req = Dma::new(BlockVirtRequest {
            ty: BlockRequestTy::In,
            reserved: 0,
            sector: block,
        })
        .unwrap();

        let result = unsafe {
            Dma::<[u8]>::zeroed_slice(target.len())
                .unwrap()
                .assume_init()
        };
        let status = Dma::new(u8::MAX).unwrap();

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&req))
            .chain(Buffer::new_unsized(&result).flags(DescriptorFlags::WRITE_ONLY))
            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        // XXX: Subtract 1 because the of status byte.
        let written = self.send(chain).await as usize - 1;
        assert_eq!(*status, 0);

        target[..written].copy_from_slice(&result);
        written
    }

    async fn write(&self, block: u64, target: &[u8]) -> usize {
        let req = Dma::new(BlockVirtRequest {
            ty: BlockRequestTy::Out,
            reserved: 0,
            sector: block,
        })
        .unwrap();

        let mut result = unsafe {
            Dma::<[u8]>::zeroed_slice(target.len())
                .unwrap()
                .assume_init()
        };
        result.copy_from_slice(target.as_ref());

        let status = Dma::new(u8::MAX).unwrap();

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&req))
            .chain(Buffer::new_sized(&result, target.len()))
            .chain(Buffer::new(&status).flags(DescriptorFlags::WRITE_ONLY))
            .build();

        self.send(chain).await as usize;
        assert_eq!(*status, 0);

        target.len()
    }
}

pub(crate) struct VirtioDisk<'a> {
    queue: Arc<Queue<'a>>,
    cfg: BlockDeviceConfig,
}

impl<'a> VirtioDisk<'a> {
    pub(crate) fn new(queue: Arc<Queue<'a>>, cfg: BlockDeviceConfig) -> Self {
        Self { queue, cfg }
    }
}

impl driver_block::Disk for VirtioDisk<'_> {
    fn block_size(&self) -> u32 {
        self.cfg.block_size()
    }

    fn size(&self) -> u64 {
        self.cfg.capacity() * u64::from(self.cfg.block_size())
    }

    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
        Ok(self.queue.read(block, buffer).await)
    }

    async fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
        Ok(self.queue.write(block, buffer).await)
    }
}
