use std::collections::{BTreeMap, HashMap};
use std::io;

use graphics_ipc::v1::Damage;
use inputd::{VtEvent, VtEventKind};
use libredox::Fd;
use redox_scheme::{RequestKind, Scheme, SignalBehavior, Socket};
use syscall::{Error, MapFlags, Result, EAGAIN, EBADF, EINVAL};

pub trait GraphicsAdapter {
    type Framebuffer: Framebuffer;

    fn displays(&self) -> Vec<usize>;
    fn display_size(&self, display_id: usize) -> (u32, u32);

    fn create_dumb_framebuffer(&mut self, width: u32, height: u32) -> Self::Framebuffer;
    fn map_dumb_framebuffer(&mut self, framebuffer: &Self::Framebuffer) -> *mut u8;

    fn update_plane(
        &mut self,
        display_id: usize,
        framebuffer: &Self::Framebuffer,
        damage: &[Damage],
    );
}

pub trait Framebuffer {
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
    vts_fb: HashMap<usize, HashMap<usize, T::Framebuffer>>,
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
            vts_fb: HashMap::new(),
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
                    let framebuffer = self
                        .vts_fb
                        .entry(vt_event.vt)
                        .or_default()
                        .entry(display_id)
                        .or_insert_with(|| {
                            let (width, height) = self.adapter.display_size(display_id);
                            self.adapter.create_dumb_framebuffer(width, height)
                        });
                    Self::update_whole_screen(&mut self.adapter, display_id, framebuffer);

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
                Err(err) => panic!("driver-graphics: failed to read display scheme: {err}"),
            };

            match request.kind() {
                RequestKind::Call(call) => {
                    let response = call.handle_scheme(self);
                    self.socket
                        .write_response(response, SignalBehavior::Restart)
                        .expect("driver-graphics: failed to write response");
                }
                RequestKind::OnClose { id } => {
                    self.on_close(id);
                }
                _ => (),
            }
        }

        Ok(())
    }

    fn update_whole_screen(adapter: &mut T, screen: usize, framebuffer: &T::Framebuffer) {
        adapter.update_plane(
            screen,
            framebuffer,
            &[Damage {
                x: 0,
                y: 0,
                width: framebuffer.width(),
                height: framebuffer.height(),
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

        self.vts_fb
            .entry(vt)
            .or_default()
            .entry(id)
            .or_insert_with(|| {
                let (width, height) = self.adapter.display_size(id);
                self.adapter.create_dumb_framebuffer(width, height)
            });

        self.next_id += 1;
        self.handles
            .insert(self.next_id, Handle::Screen { vt, screen: id });
        Ok(self.next_id)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        let framebuffer = &self.vts_fb[vt][screen];
        let path = format!(
            "{}:{vt}.{screen}/{}/{}",
            self.scheme_name,
            framebuffer.width(),
            framebuffer.height()
        );
        buf[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        if *vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the framebuffer on the next VT switch anyway
            return Ok(0);
        }
        let framebuffer = &self.vts_fb[vt][screen];
        Self::update_whole_screen(&mut self.adapter, *screen, framebuffer);
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
            // flush the framebuffer on the next VT switch anyway
            return Ok(buf.len());
        }

        let framebuffer = &self.vts_fb[vt][screen];

        let damage = unsafe {
            core::slice::from_raw_parts(
                buf.as_ptr() as *const Damage,
                buf.len() / core::mem::size_of::<Damage>(),
            )
        };

        self.adapter.update_plane(*screen, framebuffer, damage);

        Ok(buf.len())
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
        let framebuffer = &self.vts_fb[vt][screen];
        let ptr = T::map_dumb_framebuffer(&mut self.adapter, framebuffer);
        Ok(ptr as usize)
    }
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
