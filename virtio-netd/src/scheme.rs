use std::sync::Arc;

use driver_network::NetworkAdapter;

use common::dma::Dma;

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::Queue;

use crate::{VirtHeader, MAX_BUFFER_LEN};

pub struct VirtioNet<'a> {
    mac_address: [u8; 6],

    /// Reciever Queue.
    rx: Arc<Queue<'a>>,
    rx_buffers: Vec<Dma<[u8]>>,

    /// Transmiter Queue.
    tx: Arc<Queue<'a>>,

    recv_head: u16,
}

impl<'a> VirtioNet<'a> {
    pub fn new(mac_address: [u8; 6], rx: Arc<Queue<'a>>, tx: Arc<Queue<'a>>) -> Self {
        // Populate all of the `rx_queue` with buffers to maximize performence.
        let mut rx_buffers = vec![];
        for i in 0..(rx.descriptor_len() as usize) {
            rx_buffers.push(unsafe {
                Dma::<[u8]>::zeroed_slice(MAX_BUFFER_LEN)
                    .unwrap()
                    .assume_init()
            });

            let chain = ChainBuilder::new()
                .chain(Buffer::new_unsized(&rx_buffers[i]).flags(DescriptorFlags::WRITE_ONLY))
                .build();

            let _ = rx.send(chain);
        }

        Self {
            mac_address,

            rx,
            rx_buffers,
            tx,

            recv_head: 0,
        }
    }

    /// Returns the number of bytes read. Returns `0` if the operation would block.
    fn try_recv(&mut self, target: &mut [u8]) -> usize {
        let header_size = core::mem::size_of::<VirtHeader>();

        if self.recv_head == self.rx.used.head_index() {
            // The read would block.
            return 0;
        }

        let idx = self.rx.used.head_index() as usize;
        let element = self.rx.used.get_element_at(idx - 1);

        let descriptor_idx = element.table_index.get();
        let payload_size = element.written.get() as usize - header_size;

        // XXX: The header and packet are added as one output descriptor to the transmit queue,
        //      and the device is notified of the new entry (see 5.1.5 Device Initialization).
        let buffer = &self.rx_buffers[descriptor_idx as usize];
        // TODO: Check the header.
        let _header = unsafe { &*(buffer.as_ptr() as *const VirtHeader) };
        let packet = &buffer[header_size..(header_size + payload_size)];

        // Copy the packet into the buffer.
        target[..payload_size].copy_from_slice(&packet);

        self.recv_head = self.rx.used.head_index();
        payload_size
    }
}

impl<'a> NetworkAdapter for VirtioNet<'a> {
    fn mac_address(&mut self) -> [u8; 6] {
        self.mac_address
    }

    fn available_for_read(&mut self) -> usize {
        (self.rx.used.head_index() - self.recv_head).into()
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        let bytes = self.try_recv(buf);

        if bytes != 0 {
            // We read some bytes.
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    fn write_packet(&mut self, buffer: &[u8]) -> syscall::Result<usize> {
        let header = unsafe { Dma::<VirtHeader>::zeroed()?.assume_init() };

        let mut payload = unsafe { Dma::<[u8]>::zeroed_slice(buffer.len())?.assume_init() };
        payload.copy_from_slice(buffer);

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&header))
            .chain(Buffer::new_unsized(&payload))
            .build();

        futures::executor::block_on(self.tx.send(chain));
        Ok(buffer.len())
    }
}
