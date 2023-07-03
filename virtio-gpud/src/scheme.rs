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

use crate::{CommandTy, ControlHeader, GetDisplayInfo};

pub enum Handle {
    Input(InputHandle),
    Screen(ScreenHandle),
}

pub struct InputHandle {}
pub struct ScreenHandle {
    id: usize,
}

pub struct Display<'a> {
    control_queue: Arc<Queue<'a>>,
    cursor_queue: Arc<Queue<'a>>,

    display_id: usize,
    handles: BTreeMap<usize, Handle>,
    next_id: AtomicUsize,
}

impl<'a> Display<'a> {
    pub fn new(control_queue: Arc<Queue<'a>>, cursor_queue: Arc<Queue<'a>>) -> Self {
        Self {
            control_queue,
            cursor_queue,

            display_id: 0,
            handles: BTreeMap::new(),
            next_id: AtomicUsize::new(0),
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
}

impl<'a> SchemeMut for Display<'a> {
    fn open(&mut self, path: &str, flags: usize, uid: u32, gid: u32) -> syscall::Result<usize> {
        if path == "input" {
            if uid != 0 {
                return Err(SysError::new(EPERM));
            }

            let fd = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.handles.insert(fd, Handle::Input(InputHandle {}));

            Ok(fd)
        } else {
            let mut parts = path.split('/');
            let screen = parts.next().unwrap_or("").split('.');
            dbg!(screen);

            todo!();
        }
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        todo!()
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(SysError::new(EINVAL))?;

        match handle {
            Handle::Input(_) => todo!(),
            Handle::Screen(_) => {
                let size = buf.len() / core::mem::size_of::<Event>();
                let events =
                    unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<Event>(), size) };

                dbg!(events);
                todo!()
            }
        }
    }
}
