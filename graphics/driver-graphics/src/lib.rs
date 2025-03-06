use std::collections::{BTreeMap, HashMap};
use std::io;

use graphics_ipc::v1::Damage;
use inputd::{VtEvent, VtEventKind};
use libredox::errno::EOPNOTSUPP;
use libredox::Fd;
use redox_scheme::{RequestKind, Response, Scheme, SignalBehavior, Socket};
use syscall::{Error, MapFlags, Result, EAGAIN, EBADF, EINVAL};

pub trait GraphicsAdapter {
    type Resource: Resource;

    fn displays(&self) -> Vec<usize>;
    fn display_size(&self, display_id: usize) -> (u32, u32);

    fn create_resource(&mut self, width: u32, height: u32) -> Self::Resource;
    fn map_resource(&mut self, resource: &Self::Resource) -> *mut u8;

    fn update_plane(&mut self, display_id: usize, resource: &Self::Resource, damage: &[Damage]);
}

pub trait Resource {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
}

pub struct GraphicsScheme<T: GraphicsAdapter> {
    adapter: T,

    scheme_name: String,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,

    active_vt: usize,
    vts_res: HashMap<usize, HashMap<usize, T::Resource>>,
}

enum Handle {
    Screen { vt: usize, screen: usize },
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    pub fn new(adapter: T, scheme_name: String) -> Self {
        assert!(scheme_name.starts_with("display"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create graphics scheme");

        GraphicsScheme {
            adapter,
            scheme_name,
            socket,
            next_id: 0,
            handles: BTreeMap::new(),
            active_vt: 0,
            vts_res: HashMap::new(),
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

    pub fn handle_vt_event(&mut self, vt_event: VtEvent) {
        match vt_event.kind {
            VtEventKind::Activate => {
                log::info!("activate {}", vt_event.vt);

                for display_id in self.adapter.displays() {
                    let resource = self
                        .vts_res
                        .entry(vt_event.vt)
                        .or_default()
                        .entry(display_id)
                        .or_insert_with(|| {
                            let (width, height) = self.adapter.display_size(display_id);
                            self.adapter.create_resource(width, height)
                        });
                    Self::update_whole_screen(&mut self.adapter, display_id, resource);

                    self.active_vt = vt_event.vt;
                }
            }

            VtEventKind::Resize => {
                log::warn!("driver-graphics: resize is not implemented yet")
            }
        }
    }

    /// Process new scheme requests.
    ///
    /// This needs to be called each time there is a new event on the scheme
    /// file.
    pub fn tick(&mut self) -> io::Result<()> {
        loop {
            let request = match self.socket.next_request(SignalBehavior::Restart) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    // Scheme likely got unmounted
                    std::process::exit(0);
                }
                Err(err) if err.errno == EAGAIN => break,
                Err(err) => panic!("vesad: failed to read display scheme: {err}"),
            };

            match request.kind() {
                RequestKind::Call(call_request) => {
                    let resp = call_request.handle_scheme(self);
                    self.socket
                        .write_response(resp, SignalBehavior::Restart)
                        .expect("vesad: failed to write display scheme");
                }
                RequestKind::SendFd(sendfd_request) => {
                    self.socket.write_response(
                        Response::for_sendfd(&sendfd_request, Err(syscall::Error::new(EOPNOTSUPP))),
                        SignalBehavior::Restart,
                    )?;
                }
                RequestKind::Cancellation(_cancellation_request) => {}
                RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                    unreachable!()
                }
            }
        }

        Ok(())
    }

    fn update_whole_screen(adapter: &mut T, screen: usize, resource: &T::Resource) {
        adapter.update_plane(
            screen,
            resource,
            &[Damage {
                x: 0,
                y: 0,
                width: resource.width() as i32,
                height: resource.height() as i32,
            }],
        );
    }
}

impl<T: GraphicsAdapter> Scheme for GraphicsScheme<T> {
    fn open(&mut self, path: &str, _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        if path.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let mut parts = path.split('/');
        let mut screen = parts.next().unwrap_or("").split('.');

        let vt = screen.next().unwrap_or("").parse::<usize>().unwrap();
        let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

        dbg!(vt, id);

        if id >= self.adapter.displays().len() {
            return Err(Error::new(EINVAL));
        }

        self.vts_res
            .entry(vt)
            .or_default()
            .entry(id)
            .or_insert_with(|| {
                let (width, height) = self.adapter.display_size(id);
                self.adapter.create_resource(width, height)
            });

        self.next_id += 1;
        self.handles
            .insert(self.next_id, Handle::Screen { vt, screen: id });
        Ok(self.next_id)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        let resource = &self.vts_res[vt][screen];
        let path = format!(
            "{}:{vt}.{screen}/{}/{}",
            self.scheme_name,
            resource.width(),
            resource.height()
        );
        buf[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        if *vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the resource on the next VT switch anyway
            return Ok(0);
        }
        let resource = &self.vts_res[vt][screen];
        Self::update_whole_screen(&mut self.adapter, *screen, resource);
        Ok(0)
    }

    fn read(
        &mut self,
        id: usize,
        _buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        Err(Error::new(EINVAL))
    }

    fn write(&mut self, id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if *vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the resource on the next VT switch anyway
            return Ok(buf.len());
        }

        let resource = &self.vts_res[vt][screen];

        let damage = unsafe {
            core::slice::from_raw_parts(
                buf.as_ptr() as *const Damage,
                buf.len() / core::mem::size_of::<Damage>(),
            )
        };

        self.adapter.update_plane(*screen, resource, damage);

        Ok(buf.len())
    }

    fn close(&mut self, id: usize) -> syscall::Result<usize> {
        self.handles.remove(&id).ok_or(Error::new(EBADF))?;
        Ok(0)
    }
    fn mmap_prep(
        &mut self,
        id: usize,
        offset: u64,
        size: usize,
        flags: MapFlags,
    ) -> syscall::Result<usize> {
        log::info!("KSMSG MMAP {} {:?} {} {}", id, flags, offset, size);
        let handle = self.handles.get(&id).ok_or(Error::new(EINVAL))?;
        let Handle::Screen { vt, screen } = handle;
        let resource = &self.vts_res[vt][screen];
        let ptr = T::map_resource(&mut self.adapter, resource);
        Ok(ptr as usize)
    }
}
