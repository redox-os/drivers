use std::collections::{BTreeMap, HashMap};
use std::io;

use graphics_ipc::v1::{CursorDamage, Damage, GraphicsCommand};
use inputd::{VtEvent, VtEventKind};
use libredox::Fd;
use redox_scheme::{RequestKind, Scheme, SignalBehavior, Socket};
use syscall::{Error, MapFlags, Result, EAGAIN, EBADF, EINVAL};

pub trait GraphicsAdapter {
    type Framebuffer: Framebuffer;
    type Cursor: CursorFramebuffer;

    fn displays(&self) -> Vec<usize>;
    fn display_size(&self, display_id: usize) -> (u32, u32);

    fn create_dumb_framebuffer(&mut self, width: u32, height: u32) -> Self::Framebuffer;
    fn map_dumb_framebuffer(&mut self, framebuffer: &Self::Framebuffer) -> *mut u8;

    fn update_plane(&mut self, display_id: usize, framebuffer: &Self::Framebuffer, damage: Damage);

    fn supports_hw_cursor(&self) -> bool;
    fn create_cursor_framebuffer(&mut self) -> Self::Cursor;
    fn map_cursor_framebuffer(&mut self, cursor: &Self::Cursor) -> *mut u8;
    fn handle_cursor(&mut self, cursor: &mut CursorPlane<Self::Cursor>, dirty_fb: bool);
}

pub trait Framebuffer {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
}

struct FramebufferRing<T: GraphicsAdapter> {
    buffers: Vec<FramebufferEntry<T>>,
    front_buffer: usize,
}

impl<T: GraphicsAdapter> FramebufferRing<T> {
    fn new(buffer: T::Framebuffer) -> Self {
        let mut buffers = Vec::new();
        buffers.push(FramebufferEntry::new(buffer));
        Self {
            buffers,
            front_buffer: 0,
        }
    }

    fn add_buffer(&mut self, buffer: T::Framebuffer) {
        self.buffers.push(FramebufferEntry::new(buffer));
    }

    fn current_front(&mut self) -> &mut FramebufferEntry<T> {
        &mut self.buffers[self.front_buffer]
    }

    fn find_available_buffer(&mut self) -> Option<usize> {
        for (i, entry) in self.buffers.iter().enumerate() {
            match entry.state {
                BufferState::Available => {
                    return Some(i);
                }
                BufferState::Rendering => {
                    continue;
                }
                BufferState::ReadyForDisplay => {
                    continue;
                }
                BufferState::Displaying => {
                    continue;
                }
            }
        }
        None
    }
}

struct FramebufferEntry<T: GraphicsAdapter> {
    buffer: T::Framebuffer,
    state: BufferState,
}

impl<T: GraphicsAdapter> FramebufferEntry<T> {
    fn new(buffer: T::Framebuffer) -> Self {
        Self {
            buffer,
            state: BufferState::Available,
        }
    }

    fn set_state(&mut self, state: BufferState) {
        self.state = state;
    }

    fn get_buffer(&self) -> &T::Framebuffer {
        &self.buffer
    }
}

enum BufferState {
    Available,
    Rendering,
    ReadyForDisplay,
    Displaying,
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
    vts_fb: HashMap<usize, HashMap<usize, FramebufferRing<T>>>,
    cursor_planes: HashMap<usize, CursorPlane<T::Cursor>>,
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
            cursor_planes: HashMap::new(),
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
                            FramebufferRing::new(
                                self.adapter.create_dumb_framebuffer(width, height),
                            )
                        });

                    framebuffer
                        .current_front()
                        .set_state(BufferState::Displaying);

                    Self::update_whole_screen(
                        &mut self.adapter,
                        display_id,
                        framebuffer.current_front().get_buffer(),
                    );

                    self.active_vt = vt_event.vt;

                    if self.adapter.supports_hw_cursor() {
                        let cursor_plane = Self::cursor_plane_for_vt(
                            &mut self.adapter,
                            &mut self.cursor_planes,
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

    fn cursor_plane_for_vt<'a>(
        adapter: &mut T,
        cursor_planes: &'a mut HashMap<usize, CursorPlane<T::Cursor>>,
        vt: usize,
    ) -> &'a mut CursorPlane<T::Cursor> {
        cursor_planes.entry(vt).or_insert_with(|| CursorPlane {
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

        if id >= self.adapter.displays().len() {
            return Err(Error::new(EINVAL));
        }

        self.vts_fb
            .entry(vt)
            .or_default()
            .entry(id)
            .or_insert_with(|| {
                let (width, height) = self.adapter.display_size(id);
                FramebufferRing::new(self.adapter.create_dumb_framebuffer(width, height))
            });

        self.next_id += 1;
        self.handles
            .insert(self.next_id, Handle::Screen { vt, screen: id });
        Ok(self.next_id)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let framebuffer = &mut self
            .vts_fb
            .get_mut(vt)
            .and_then(|vt_map| vt_map.get_mut(screen))
            .expect("Framebuffer not found")
            .current_front()
            .get_buffer();

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

        let framebuffer = &mut self
            .vts_fb
            .get_mut(vt)
            .and_then(|vt_map| vt_map.get_mut(screen))
            .expect("Framebuffer not found")
            .current_front()
            .get_buffer();

        Self::update_whole_screen(&mut self.adapter, *screen, framebuffer);
        Ok(0)
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        let _handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        //Currently read is only used for Orbital to check GPU cursor support
        //and only expects a buf to pass a 0 or 1 flag
        if self.adapter.supports_hw_cursor() {
            buf[0] = 1;
        } else {
            buf[0] = 0;
        }

        Ok(1)
    }

    fn write(&mut self, id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
        if buf.len() < std::mem::size_of::<u32>() {
            return Err(Error::new(EINVAL));
        }

        let Handle::Screen { vt, screen } = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        if *vt != self.active_vt {
            // This is a protection against background VT's spamming us with flush requests. We will
            // flush the framebuffer on the next VT switch anyway
            return Ok(buf.len());
        }

        let command = GraphicsCommand::command_type(&buf);

        match command {
            GraphicsCommand::UpdateDisplay(damage) => {
                let framebuffer = &mut self
                    .vts_fb
                    .get_mut(vt)
                    .and_then(|vt_map| vt_map.get_mut(screen))
                    .expect("Framebuffer not found")
                    .current_front()
                    .get_buffer();

                self.adapter.update_plane(*screen, framebuffer, damage);
            }
            GraphicsCommand::UpdateCursor(cursor_damage) => {
                if !self.adapter.supports_hw_cursor() {
                    return Err(Error::new(EINVAL));
                }

                let cursor_plane = Self::cursor_plane_for_vt(
                    &mut self.adapter,
                    &mut self.cursor_planes,
                    self.active_vt,
                );

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
            }
            GraphicsCommand::CreateFramebuffer(create_framebuffer) => {
                let framebuffer = self
                    .vts_fb
                    .get_mut(vt)
                    .and_then(|vt_map| vt_map.get_mut(screen))
                    .expect("Framebuffer not found");

                framebuffer.add_buffer(
                    self.adapter.create_dumb_framebuffer(
                        create_framebuffer.width,
                        create_framebuffer.height,
                    ),
                );
            }
            _ => {
                return Err(Error::new(EINVAL));
            }
        }

        Ok(buf.len())
    }

    fn mmap_prep(
        &mut self,
        id: usize,
        _offset: u64,
        _size: usize,
        _flags: MapFlags,
    ) -> syscall::Result<usize> {
        // log::trace!("KSMSG MMAP {} {:?} {} {}", id, _flags, _offset, _size);
        let handle = self.handles.get(&id).ok_or(Error::new(EINVAL))?;
        let Handle::Screen { vt, screen } = handle;
        let framebuffer = &mut self
            .vts_fb
            .get_mut(vt)
            .and_then(|vt_map| vt_map.get_mut(screen))
            .expect("Framebuffer not found")
            .current_front()
            .get_buffer();
        let ptr = T::map_dumb_framebuffer(&mut self.adapter, framebuffer);
        Ok(ptr as usize)
    }
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
