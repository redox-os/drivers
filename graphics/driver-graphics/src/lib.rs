use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;

use graphics_ipc::v1::{CursorDamage, Damage};
use inputd::{VtEvent, VtEventKind};
use libredox::Fd;
use redox_scheme::{RequestKind, Scheme, SignalBehavior, Socket};
use syscall::{Error, MapFlags, Result, EAGAIN, EBADF, EINVAL};

pub trait GraphicsAdapter {
    type Framebuffer: Framebuffer;
    type Cursor: CursorFramebuffer;

    fn display_count(&self) -> usize;
    fn display_size(&self, display_id: usize) -> (u32, u32);

    fn create_dumb_framebuffer(&mut self, width: u32, height: u32) -> Self::Framebuffer;
    fn map_dumb_framebuffer(&mut self, framebuffer: &Self::Framebuffer) -> *mut u8;

    fn update_plane(&mut self, display_id: usize, framebuffer: &Self::Framebuffer, damage: Damage);

    fn supports_hw_cursor(&self) -> bool;
    fn create_cursor_framebuffer(&mut self) -> Self::Cursor;
    fn map_cursor_framebuffer(&mut self, cursor: &Self::Cursor) -> *mut u8;
    fn handle_cursor(&mut self, cursor: &CursorPlane<Self::Cursor>, dirty_fb: bool);
}

pub trait Framebuffer {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
}

pub struct CursorPlane<C: CursorFramebuffer> {
    pub x: i32,
    pub y: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub framebuffer: C,
}

pub trait CursorFramebuffer {}

pub struct GraphicsScheme<T: GraphicsAdapter> {
    adapter: T,

    scheme_name: String,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle>,

    active_vt: usize,
    vts: HashMap<usize, VtState<T>>,
}

struct VtState<T: GraphicsAdapter> {
    display_fbs: HashMap<usize, Arc<T::Framebuffer>>,
    cursor_plane: Option<CursorPlane<T::Cursor>>,
}

impl<T: GraphicsAdapter> Default for VtState<T> {
    fn default() -> Self {
        VtState {
            display_fbs: HashMap::default(),
            cursor_plane: None,
        }
    }
}

enum Handle {
    V1Screen { vt: usize, screen: usize },
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
            vts: HashMap::new(),
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

                for display_id in 0..self.adapter.display_count() {
                    let framebuffer = Self::framebuffer_for_vt_and_display(
                        &mut self.adapter,
                        &mut self.vts,
                        vt_event.vt,
                        display_id,
                    );
                    Self::update_whole_screen(&mut self.adapter, display_id, framebuffer);

                    self.active_vt = vt_event.vt;

                    if self.adapter.supports_hw_cursor() {
                        let cursor_plane = Self::cursor_plane_for_vt(
                            &mut self.adapter,
                            &mut self.vts,
                            self.active_vt,
                        );
                        self.adapter.handle_cursor(cursor_plane, true);
                    }
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
            Damage {
                x: 0,
                y: 0,
                width: framebuffer.width(),
                height: framebuffer.height(),
            },
        );
    }

    fn framebuffer_for_vt_and_display<'a>(
        adapter: &mut T,
        vts: &'a mut HashMap<usize, VtState<T>>,
        vt: usize,
        display_id: usize,
    ) -> &'a T::Framebuffer {
        vts.entry(vt)
            .or_default()
            .display_fbs
            .entry(display_id)
            .or_insert_with(|| {
                let (width, height) = adapter.display_size(display_id);
                Arc::new(adapter.create_dumb_framebuffer(width, height))
            })
    }

    fn cursor_plane_for_vt<'a>(
        adapter: &mut T,
        vts: &'a mut HashMap<usize, VtState<T>>,
        vt: usize,
    ) -> &'a mut CursorPlane<T::Cursor> {
        vts.entry(vt)
            .or_default()
            .cursor_plane
            .get_or_insert_with(|| CursorPlane {
                x: 0,
                y: 0,
                hot_x: 0,
                hot_y: 0,
                framebuffer: adapter.create_cursor_framebuffer(),
            })
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

        if id >= self.adapter.display_count() {
            return Err(Error::new(EINVAL));
        }

        Self::framebuffer_for_vt_and_display(&mut self.adapter, &mut self.vts, vt, id);

        self.next_id += 1;
        self.handles
            .insert(self.next_id, Handle::V1Screen { vt, screen: id });
        Ok(self.next_id)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let path = match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                let framebuffer = &self.vts[vt].display_fbs[screen];
                format!(
                    "{}:{vt}.{screen}/{}/{}",
                    self.scheme_name,
                    framebuffer.width(),
                    framebuffer.height()
                )
            }
        };
        buf[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                if *vt != self.active_vt {
                    // This is a protection against background VT's spamming us with flush requests. We will
                    // flush the framebuffer on the next VT switch anyway
                    return Ok(0);
                }
                let framebuffer = &self.vts[vt].display_fbs[screen];
                Self::update_whole_screen(&mut self.adapter, *screen, framebuffer);
                Ok(0)
            }
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { .. } => {
                //Currently read is only used for Orbital to check GPU cursor support
                //and only expects a buf to pass a 0 or 1 flag
                if self.adapter.supports_hw_cursor() {
                    buf[0] = 1;
                } else {
                    buf[0] = 0;
                }

                Ok(1)
            }
        }
    }

    fn write(&mut self, id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                if *vt != self.active_vt {
                    // This is a protection against background VT's spamming us with flush requests. We will
                    // flush the framebuffer on the next VT switch anyway
                    return Ok(buf.len());
                }

                if size_of_val(buf) == std::mem::size_of::<CursorDamage>()
                    && self.adapter.supports_hw_cursor()
                {
                    let cursor_damage = unsafe { *buf.as_ptr().cast::<CursorDamage>() };

                    let cursor_plane =
                        Self::cursor_plane_for_vt(&mut self.adapter, &mut self.vts, self.active_vt);

                    cursor_plane.x = cursor_damage.x;
                    cursor_plane.y = cursor_damage.y;

                    if cursor_damage.header == 0 {
                        self.adapter.handle_cursor(cursor_plane, false);
                    } else {
                        cursor_plane.hot_x = cursor_damage.hot_x;
                        cursor_plane.hot_y = cursor_damage.hot_y;

                        let w: i32 = cursor_damage.width;
                        let h: i32 = cursor_damage.height;
                        let cursor_image = cursor_damage.cursor_img_bytes;
                        let cursor_ptr = self
                            .adapter
                            .map_cursor_framebuffer(&cursor_plane.framebuffer);

                        //Clear previous image from backing storage
                        unsafe {
                            core::ptr::write_bytes(cursor_ptr as *mut u8, 0, 64 * 64 * 4);
                        }

                        //Write image to backing storage
                        for row in 0..h {
                            let start: usize = (w * row) as usize;
                            let end: usize = (w * row + w) as usize;

                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    cursor_image[start..end].as_ptr(),
                                    cursor_ptr.cast::<u32>().offset(64 * row as isize),
                                    w as usize,
                                );
                            }
                        }

                        self.adapter.handle_cursor(cursor_plane, true);
                    }

                    return Ok(buf.len());
                }

                let framebuffer = &self.vts[vt].display_fbs[screen];

                assert_eq!(buf.len(), std::mem::size_of::<Damage>());
                let damage = unsafe { *buf.as_ptr().cast::<Damage>() };

                self.adapter.update_plane(*screen, framebuffer, damage);

                Ok(buf.len())
            }
        }
    }

    fn mmap_prep(
        &mut self,
        id: usize,
        _offset: u64,
        _size: usize,
        _flags: MapFlags,
    ) -> syscall::Result<usize> {
        // log::trace!("KSMSG MMAP {} {:?} {} {}", id, _flags, _offset, _size);
        let framebuffer = match self.handles.get(&id).ok_or(Error::new(EINVAL))? {
            Handle::V1Screen { vt, screen } => &self.vts[vt].display_fbs[screen],
        };
        let ptr = T::map_dumb_framebuffer(&mut self.adapter, framebuffer);
        Ok(ptr as usize)
    }
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
