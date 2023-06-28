use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use syscall::Error as SysError;
use syscall::*;

use virtio_core::spec::{Buffer, ChainBuilder, DescriptorFlags};
use virtio_core::transport::Queue;

use crate::{VirtHeader, MAX_BUFFER_LEN};

pub struct NetworkScheme<'a> {
    /// Reciever Queue.
    rx: Arc<Queue<'a>>,
    rx_buffers: Vec<Dma<[u8]>>,

    /// Transmiter Queue.
    tx: Arc<Queue<'a>>,
    /// File descriptor handles.
    handles: BTreeMap<usize, usize>,
    next_id: AtomicUsize,

    recv_head: u16,
}

impl<'a> NetworkScheme<'a> {
    pub fn new(rx: Arc<Queue<'a>>, tx: Arc<Queue<'a>>) -> Self {
        // Populate all of the `rx_queue` with buffers to maximize performence.
        let mut rx_buffers = vec![];
        for i in 0..(rx.descriptor_len() as usize) {
            rx_buffers.push(unsafe { Dma::<[u8]>::zeroed_unsized(MAX_BUFFER_LEN) }.unwrap());

            let chain = ChainBuilder::new()
                .chain(Buffer::new_unsized(&rx_buffers[i]).flags(DescriptorFlags::WRITE_ONLY))
                .build();

            rx.send(chain);
        }

        Self {
            rx,
            rx_buffers,
            tx,

            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),

            recv_head: 0,
        }
    }

    /// Returns the number of bytes read. Returns `0` if the operation would block.
    fn try_recv(&mut self, target: &mut [u8]) -> usize {
        let header_size = core::mem::size_of::<VirtHeader>();

        let mut queue = self.rx.inner.lock().unwrap();

        if self.recv_head == queue.used.head_index() {
            // The read would block.
            return 0;
        }

        let idx = queue.used.head_index() as usize;
        let element = queue.used.get_element_at(idx - 1);

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

        self.recv_head = queue.used.head_index();
        payload_size
    }
}

impl<'a> SchemeBlockMut for NetworkScheme<'a> {
    fn open(
        &mut self,
        _path: &str,
        flags: usize,
        uid: u32,
        _gid: u32,
    ) -> syscall::Result<Option<usize>> {
        if uid != 0 {
            return Err(SysError::new(EACCES));
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.handles.insert(id, flags);

        Ok(Some(id))
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        let flags = *self.handles.get(&id).ok_or(SysError::new(EBADF))?;
        let bytes = self.try_recv(buf);

        if bytes != 0 {
            // We read some bytes.
            Ok(Some(bytes))
        } else if flags & O_NONBLOCK == O_NONBLOCK {
            // We are in non-blocking mode.
            Err(SysError::new(EWOULDBLOCK))
        } else {
            // Block
            unimplemented!()
        }
    }

    fn write(&mut self, id: usize, buffer: &[u8]) -> syscall::Result<Option<usize>> {
        if self.handles.get(&id).is_none() {
            return Err(SysError::new(EBADF));
        }

        let header = unsafe { Dma::<VirtHeader>::zeroed()?.assume_init() };

        // TODO: Does the payload actually need to be a DMA buffer?
        let mut payload = unsafe { Dma::<[u8]>::zeroed_unsized(buffer.len())? };
        payload.copy_from_slice(buffer);

        let chain = ChainBuilder::new()
            .chain(Buffer::new(&header))
            .chain(Buffer::new_unsized(&payload))
            .build();

        self.tx.send(chain);
        core::mem::forget(payload);

        Ok(Some(buffer.len()))
    }

    fn dup(&mut self, _old_id: usize, _buf: &[u8]) -> syscall::Result<Option<usize>> {
        unimplemented!()
    }

    fn fevent(
        &mut self,
        id: usize,
        _flags: syscall::EventFlags,
    ) -> syscall::Result<Option<syscall::EventFlags>> {
        let _flags = self.handles.get(&id).ok_or(SysError::new(EBADF))?;
        Ok(Some(syscall::EventFlags::empty()))
    }

    fn fpath(&mut self, _id: usize, _buf: &mut [u8]) -> syscall::Result<Option<usize>> {
        unimplemented!()
    }

    fn fsync(&mut self, _id: usize) -> syscall::Result<Option<usize>> {
        unimplemented!()
    }

    fn close(&mut self, _id: usize) -> syscall::Result<Option<usize>> {
        unimplemented!()
    }
}
