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

pub trait USBGadget {
}

enum Handle {
    Data,
}

pub struct USBGadgetScheme<T: USBGadget> {
    gadget: T,
    scheme_name: String,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,
}

impl<T: USBGadget> USBGadgetScheme<T> {
    pub fn new(gadget: T, scheme_name: String) -> Self {
        assert!(scheme_name.starts_with("gadget"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create Gadget scheme");

        USBGadgetScheme {
            gadget,
            scheme_name,
            socket,
            next_id: 0,
            handles: BTreeMap::new(),
        }
    }

    pub fn event_handle(&self) -> &Fd {
        self.socket.inner()
    }

    pub fn gadget(&self) -> &T {
        &self.gadget
    }

    pub fn gadget_mut(&mut self) -> &mut T {
        &mut self.gadget
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

impl<T: USBGadget>SchemeAsync for USBGadgetScheme<T> {
}
