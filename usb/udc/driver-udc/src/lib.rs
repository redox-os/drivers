use std::collections::BTreeMap;
use std::{cmp, io};

use libredox::flag::O_NONBLOCK;
use libredox::Fd;
use redox_scheme::scheme::SchemeAsync;
use redox_scheme::{
    CallRequest, CallerCtx, OpenResult, RequestKind, Response, SignalBehavior, Socket,
};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, EventFlags, Result, Stat, EACCES, EAGAIN, EBADF, EINTR, EINVAL, EWOULDBLOCK, MODE_FILE,
};

pub trait UDCAdapter {
    fn write_ep(&mut self, ep: usize, buf: &[u8]) -> Result<usize>;
    fn read_ep(&mut self, ep: usize, buf: &mut [u8]) -> Result<Option<usize>>;
}

enum Handle {
    Data,
}

pub struct UDCScheme<T: UDCAdapter> {
    udc: T,
    scheme_name: String,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,    
}

impl<T: UDCAdapter> UDCScheme<T> {
    pub fn new(udc: T, scheme_name: String) -> Self {
        assert!(scheme_name.starts_with("udc"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create UDC scheme");

        UDCScheme {
            udc,
            scheme_name,
            socket,
            next_id: 0,
            handles: BTreeMap::new(),
        }
    }

    pub fn event_handle(&self) -> &Fd {
        self.socket.inner()
    }

    pub fn udc(&self) -> &T {
        &self.udc
    }

    pub fn udc_mut(&mut self) -> &mut T {
        &mut self.udc
    }

    pub fn tick(&mut self) -> io::Result<()> {
        loop {
            let request = match self.socket.next_request(SignalBehavior::Restart) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    // Scheme likely got unmounted
                    std::process::exit(0);
                }
                Err(err) if err.errno == EAGAIN => break,
                Err(err) => return Err(err.into()),
            };

            match request.kind() {
                _=> todo!("Not yet implemented"),
            }
        }

        Ok(())
    }

    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}

impl<T: UDCAdapter>SchemeAsync for UDCScheme<T> {
}
