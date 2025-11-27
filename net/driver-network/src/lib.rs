use std::collections::BTreeMap;
use std::{cmp, io};

use libredox::flag::O_NONBLOCK;
use libredox::Fd;
use redox_scheme::{
    CallRequest, CallerCtx, OpenResult, RequestKind, Response, SchemeBlock, SignalBehavior, Socket,
};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, EventFlags, Result, Stat, EACCES, EAGAIN, EBADF, EINTR, EINVAL, EWOULDBLOCK, MODE_FILE,
};

pub trait NetworkAdapter {
    /// The [MAC address](https://en.wikipedia.org/wiki/MAC_address) of this
    /// network adapter.
    fn mac_address(&mut self) -> [u8; 6];

    /// The amount of network packets that can be read without blocking.
    fn available_for_read(&mut self) -> usize;

    /// Attempt to read a network packet without blocking.
    ///
    /// Returns `Ok(None)` when there is no pending network packet.
    fn read_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>>;

    /// Write a single network packet.
    // FIXME support back pressure on writes by returning EWOULDBLOCK or not
    // returning from the write syscall until there is room.
    fn write_packet(&mut self, buf: &[u8]) -> Result<usize>;
}

pub struct NetworkScheme<T: NetworkAdapter> {
    adapter: T,
    scheme_name: String,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,
    blocked: Vec<CallRequest>,
}

enum Handle {
    Data,
    Mac,
}

impl<T: NetworkAdapter> NetworkScheme<T> {
    pub fn new(
        adapter_fn: impl FnOnce() -> T,
        daemon: redox_daemon::Daemon,
        scheme_name: String,
    ) -> Self {
        assert!(scheme_name.starts_with("network"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create network scheme");
        daemon.ready().expect("failed to mark daemon as ready");
        let adapter = adapter_fn();
        NetworkScheme {
            adapter,
            scheme_name,
            socket,
            next_id: 0,
            handles: BTreeMap::new(),
            blocked: vec![],
        }
    }

    pub fn event_handle(&self) -> &Fd {
        self.socket.inner()
    }

    pub fn adapter(&self) -> &T {
        &self.adapter
    }

    pub fn adapter_mut(&mut self) -> &mut T {
        &mut self.adapter
    }

    /// Process pending and new requests.
    ///
    /// This needs to be called each time there is a new event on the scheme
    /// file and each time a new network packet has been received by the
    /// driver.
    // FIXME maybe split into one method for events on the scheme fd and one
    // to call when an irq is received to indicate that blocked requests can
    // be processed.
    pub fn tick(&mut self) -> io::Result<()> {
        // Handle any blocked requests
        let mut i = 0;
        while i < self.blocked.len() {
            if let Some(resp) = self.blocked[i].handle_scheme_block(self) {
                self.socket
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("driver-network: failed to write scheme");
                self.blocked.remove(i);
            } else {
                i += 1;
            }
        }

        // Handle new scheme requests
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
                RequestKind::Call(call_request) => {
                    if let Some(resp) = call_request.handle_scheme_block(self) {
                        self.socket.write_response(resp, SignalBehavior::Restart)?;
                    } else {
                        self.blocked.push(call_request);
                    }
                }
                RequestKind::OnClose { id } => {
                    self.on_close(id);
                }
                RequestKind::Cancellation(cancellation_request) => {
                    if let Some(i) = self
                        .blocked
                        .iter()
                        .position(|req| req.request().request_id() == cancellation_request.id)
                    {
                        let blocked_req = self.blocked.remove(i);
                        let resp = Response::new(&blocked_req, Err(syscall::Error::new(EINTR)));
                        self.socket.write_response(resp, SignalBehavior::Restart)?;
                    }
                }
                _ => {}
            }
        }

        // Notify readers about incoming events
        let available_for_read = self.adapter.available_for_read();
        if available_for_read > 0 {
            for &handle_id in self.handles.keys() {
                self.socket
                    .post_fevent(handle_id, syscall::flag::EVENT_READ.bits())?;
            }
            return Ok(());
        }

        Ok(())
    }
}

impl<T: NetworkAdapter> SchemeBlock for NetworkScheme<T> {
    fn xopen(
        &mut self,
        path: &str,
        _flags: usize,
        caller_ctx: &CallerCtx,
    ) -> Result<Option<OpenResult>> {
        if caller_ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }

        let (handle, flags) = match path {
            "" => (Handle::Data, NewFdFlags::empty()),
            "mac" => (Handle::Mac, NewFdFlags::POSITIONED),
            _ => return Err(Error::new(EINVAL)),
        };

        self.next_id += 1;
        self.handles.insert(self.next_id, handle);
        Ok(Some(OpenResult::ThisScheme {
            number: self.next_id,
            flags,
        }))
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        match *handle {
            Handle::Data => {}
            Handle::Mac => {
                let data = &self.adapter.mac_address()[offset as usize..];
                let i = cmp::min(buf.len(), data.len());
                buf[..i].copy_from_slice(&data[..i]);
                return Ok(Some(i));
            }
        };

        match self.adapter.read_packet(buf)? {
            Some(count) => Ok(Some(count)),
            None => {
                if fcntl_flags & O_NONBLOCK as u32 != 0 {
                    Err(Error::new(EWOULDBLOCK))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        match handle {
            Handle::Data => {}
            Handle::Mac { .. } => return Err(Error::new(EINVAL)),
        }

        Ok(Some(self.adapter.write_packet(buf)?))
    }

    fn fevent(&mut self, id: usize, _flags: EventFlags) -> Result<Option<EventFlags>> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        Ok(Some(EventFlags::empty()))
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;

        let scheme_name = self.scheme_name.as_bytes();
        let mut j = 0;
        while i < buf.len() && j < scheme_name.len() {
            buf[i] = scheme_name[j];
            i += 1;
            j += 1;
        }

        if i < buf.len() {
            buf[i] = b':';
            i += 1;
        }

        let path = match handle {
            Handle::Data { .. } => &b""[..],
            Handle::Mac { .. } => &b"mac"[..],
        };

        j = 0;
        while i < buf.len() && j < path.len() {
            buf[i] = path[j];
            i += 1;
            j += 1;
        }

        Ok(Some(i))
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        match handle {
            Handle::Data { .. } => {
                stat.st_mode = MODE_FILE | 0o700;
            }
            Handle::Mac { .. } => {
                stat.st_mode = MODE_FILE | 0o400;
                stat.st_size = 6;
            }
        }

        Ok(Some(0))
    }

    fn fsync(&mut self, id: usize) -> Result<Option<usize>> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        Ok(Some(0))
    }
}

impl<T: NetworkAdapter> NetworkScheme<T> {
    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
