#![feature(slice_as_array)]

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Write};
use std::mem::transmute;
use std::sync::Arc;

use graphics_ipc::v1::{CursorDamage, Damage};
use inputd::{VtEvent, VtEventKind};
use libredox::Fd;
use redox_scheme::scheme::SchemeSync;
use redox_scheme::{CallerCtx, OpenResult, RequestKind, SignalBehavior, Socket};
use syscall::schemev2::NewFdFlags;
use syscall::{Error, MapFlags, Result, EAGAIN, EBADF, EINVAL, ENOENT, EOPNOTSUPP};

pub trait GraphicsAdapter {
    type Framebuffer: Framebuffer;
    type Cursor: CursorFramebuffer;

    /// The maximum amount of displays that could be attached.
    ///
    /// This must be constant for the lifetime of the graphics adapter.
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
    disable_graphical_debug: Option<File>,
    socket: Socket,
    next_id: usize,
    handles: BTreeMap<usize, Handle<T>>,

    active_vt: usize,
    vts: HashMap<usize, VtState<T>>,
}

struct VtState<T: GraphicsAdapter> {
    display_fbs: Vec<Arc<T::Framebuffer>>,
    cursor_plane: Option<CursorPlane<T::Cursor>>,
}

enum Handle<T: GraphicsAdapter> {
    V1Screen {
        vt: usize,
        screen: usize,
    },
    V2 {
        vt: usize,
        next_id: usize,
        fbs: HashMap<usize, Arc<T::Framebuffer>>,
    },
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    pub fn new(adapter: T, scheme_name: String) -> Self {
        assert!(scheme_name.starts_with("display"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create graphics scheme");

        let disable_graphical_debug = Some(
            File::open("/scheme/debug/disable-graphical-debug")
                .expect("vesad: Failed to open /scheme/debug/disable-graphical-debug"),
        );

        GraphicsScheme {
            adapter,
            scheme_name,
            disable_graphical_debug,
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

                // Disable the kernel graphical debug writing once switching vt's for the
                // first time. This way the kernel graphical debug remains enabled if the
                // userspace logging infrastructure doesn't start up because for example a
                // kernel panic happened prior to it starting up or logd crashed.
                if let Some(mut disable_graphical_debug) = self.disable_graphical_debug.take() {
                    let _ = disable_graphical_debug.write(&[1]);
                }

                self.active_vt = vt_event.vt;

                let vt_state =
                    Self::get_or_create_vt(&mut self.adapter, &mut self.vts, vt_event.vt);

                for (display_id, fb) in vt_state.display_fbs.iter().enumerate() {
                    Self::update_whole_screen(&mut self.adapter, display_id, fb);
                }

                if let Some(cursor_plane) = &vt_state.cursor_plane {
                    self.adapter.handle_cursor(cursor_plane, true);
                }
            }

            VtEventKind::Resize => {
                log::warn!("driver-graphics: resize is not implemented yet")
            }
        }
    }

    pub fn notify_displays_changed(&mut self) {
        // FIXME notify clients
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
                    let response = call.handle_sync(self);
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

    fn get_or_create_vt<'a>(
        adapter: &mut T,
        vts: &'a mut HashMap<usize, VtState<T>>,
        vt: usize,
    ) -> &'a mut VtState<T> {
        vts.entry(vt).or_insert_with(|| {
            let mut display_fbs = vec![];
            for display_id in 0..adapter.display_count() {
                let (width, height) = adapter.display_size(display_id);
                display_fbs.push(Arc::new(adapter.create_dumb_framebuffer(width, height)));
            }

            let cursor_plane = adapter.supports_hw_cursor().then(|| CursorPlane {
                x: 0,
                y: 0,
                hot_x: 0,
                hot_y: 0,
                framebuffer: adapter.create_cursor_framebuffer(),
            });

            VtState {
                display_fbs,
                cursor_plane,
            }
        })
    }
}

const MAP_FAKE_OFFSET_MULTIPLIER: usize = 0x10_000_000;

impl<T: GraphicsAdapter> SchemeSync for GraphicsScheme<T> {
    fn open(&mut self, path: &str, _flags: usize, _ctx: &CallerCtx) -> Result<OpenResult> {
        if path.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let handle = if path.starts_with("v") {
            if !path.starts_with("v2/") {
                return Err(Error::new(ENOENT));
            }
            let vt = path["v2/".len()..]
                .parse::<usize>()
                .map_err(|_| Error::new(EINVAL))?;

            // Ensure the VT exists such that the rest of the methods can freely access it.
            Self::get_or_create_vt(&mut self.adapter, &mut self.vts, vt);

            Handle::V2 {
                vt,
                next_id: 0,
                fbs: HashMap::new(),
            }
        } else {
            let mut parts = path.split('/');
            let mut screen = parts.next().unwrap_or("").split('.');

            let vt = screen.next().unwrap_or("").parse::<usize>().unwrap();
            let id = screen.next().unwrap_or("").parse::<usize>().unwrap_or(0);

            if id >= self.adapter.display_count() {
                return Err(Error::new(EINVAL));
            }

            // Ensure the VT exists such that the rest of the methods can freely access it.
            Self::get_or_create_vt(&mut self.adapter, &mut self.vts, vt);

            Handle::V1Screen { vt, screen: id }
        };
        self.next_id += 1;
        self.handles.insert(self.next_id, handle);
        Ok(OpenResult::ThisScheme {
            number: self.next_id,
            flags: NewFdFlags::empty(),
        })
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> syscall::Result<usize> {
        let path = match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                let framebuffer = &self.vts[vt].display_fbs[*screen];
                format!(
                    "{}:{vt}.{screen}/{}/{}",
                    self.scheme_name,
                    framebuffer.width(),
                    framebuffer.height()
                )
            }
            Handle::V2 {
                vt,
                next_id: _,
                fbs: _,
            } => format!("/scheme/{}/v2/{vt}", self.scheme_name),
        };
        buf[..path.len()].copy_from_slice(path.as_bytes());
        Ok(path.len())
    }

    fn fsync(&mut self, id: usize, _ctx: &CallerCtx) -> syscall::Result<()> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                if *vt != self.active_vt {
                    // This is a protection against background VT's spamming us with flush requests. We will
                    // flush the framebuffer on the next VT switch anyway
                    return Ok(());
                }
                Self::update_whole_screen(
                    &mut self.adapter,
                    *screen,
                    &self.vts[vt].display_fbs[*screen],
                );
                Ok(())
            }
            Handle::V2 { .. } => Err(Error::new(EOPNOTSUPP)),
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
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
            Handle::V2 { .. } => Err(Error::new(EOPNOTSUPP)),
        }
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { vt, screen } => {
                if *vt != self.active_vt {
                    // This is a protection against background VT's spamming us with flush requests. We will
                    // flush the framebuffer on the next VT switch anyway
                    return Ok(buf.len());
                }

                let vt_state = self.vts.get_mut(vt).unwrap();

                if size_of_val(buf) == std::mem::size_of::<CursorDamage>() {
                    let Some(cursor_plane) = &mut vt_state.cursor_plane else {
                        // Hardware cursor not supported
                        return Err(Error::new(EINVAL));
                    };

                    let cursor_damage = unsafe { *buf.as_ptr().cast::<CursorDamage>() };

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

                assert_eq!(buf.len(), std::mem::size_of::<Damage>());
                let damage = unsafe { *buf.as_ptr().cast::<Damage>() };

                self.adapter
                    .update_plane(*screen, &vt_state.display_fbs[*screen], damage);

                Ok(buf.len())
            }
            Handle::V2 { .. } => Err(Error::new(EOPNOTSUPP)),
        }
    }

    fn call(&mut self, id: usize, payload: &mut [u8], metadata: &[u64]) -> Result<usize> {
        use graphics_ipc::v2::ipc;

        match self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::V1Screen { .. } => {
                return Err(Error::new(EOPNOTSUPP));
            }
            Handle::V2 { vt, next_id, fbs } => match metadata[0] {
                ipc::DISPLAY_COUNT => {
                    if payload.len() < size_of::<ipc::DisplayCount>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<&mut [u8; size_of::<ipc::DisplayCount>()], &mut ipc::DisplayCount>(
                            payload.as_mut_array().unwrap(),
                        )
                    };
                    payload.count = self.adapter.display_count();
                    Ok(size_of::<ipc::DisplayCount>())
                }
                ipc::DISPLAY_SIZE => {
                    if payload.len() < size_of::<ipc::DisplaySize>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<&mut [u8; size_of::<ipc::DisplaySize>()], &mut ipc::DisplaySize>(
                            payload.as_mut_array().unwrap(),
                        )
                    };
                    let display_id = payload.display_id;
                    if display_id >= self.adapter.display_count() {
                        return Err(Error::new(EINVAL));
                    }
                    let (width, height) = self.adapter.display_size(display_id);
                    payload.width = width;
                    payload.height = height;
                    Ok(size_of::<ipc::DisplaySize>())
                }
                ipc::CREATE_DUMB_FRAMEBUFFER => {
                    if payload.len() < size_of::<ipc::CreateDumbFramebuffer>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<
                            &mut [u8; size_of::<ipc::CreateDumbFramebuffer>()],
                            &mut ipc::CreateDumbFramebuffer,
                        >(payload.as_mut_array().unwrap())
                    };

                    let fb = self
                        .adapter
                        .create_dumb_framebuffer(payload.width, payload.height);

                    *next_id += 1;
                    fbs.insert(*next_id, Arc::new(fb));
                    payload.fb_id = *next_id;
                    Ok(size_of::<ipc::CreateDumbFramebuffer>())
                }
                ipc::DUMB_FRAMEBUFFER_MAP_OFFSET => {
                    if payload.len() < size_of::<ipc::DumbFramebufferMapOffset>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<
                            &mut [u8; size_of::<ipc::DumbFramebufferMapOffset>()],
                            &mut ipc::DumbFramebufferMapOffset,
                        >(payload.as_mut_array().unwrap())
                    };

                    let fb_id = payload.fb_id;

                    if !fbs.contains_key(&fb_id) {
                        return Err(Error::new(EINVAL));
                    }

                    // FIXME use a better scheme for creating map offsets
                    assert!(
                        ((fbs[&fb_id].width() * fbs[&fb_id].height() * 4) as usize)
                            < MAP_FAKE_OFFSET_MULTIPLIER
                    );

                    payload.offset = fb_id * MAP_FAKE_OFFSET_MULTIPLIER;

                    Ok(size_of::<ipc::DumbFramebufferMapOffset>())
                }
                ipc::DESTROY_DUMB_FRAMEBUFFER => {
                    if payload.len() < size_of::<ipc::DestroyDumbFramebuffer>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<
                            &mut [u8; size_of::<ipc::DestroyDumbFramebuffer>()],
                            &mut ipc::DestroyDumbFramebuffer,
                        >(payload.as_mut_array().unwrap())
                    };

                    if fbs.remove(&{ payload.fb_id }).is_none() {
                        return Err(Error::new(ENOENT));
                    }

                    Ok(size_of::<ipc::DestroyDumbFramebuffer>())
                }
                ipc::UPDATE_PLANE => {
                    if payload.len() < size_of::<ipc::UpdatePlane>() {
                        return Err(Error::new(EINVAL));
                    }
                    let payload = unsafe {
                        transmute::<&mut [u8; size_of::<ipc::UpdatePlane>()], &mut ipc::UpdatePlane>(
                            payload.as_mut_array().unwrap(),
                        )
                    };

                    let display_id = payload.display_id;
                    if display_id >= self.adapter.display_count() {
                        return Err(Error::new(EINVAL));
                    }

                    let Some(framebuffer) = fbs.get(&{ payload.fb_id }) else {
                        return Err(Error::new(EINVAL));
                    };

                    self.vts.get_mut(vt).unwrap().display_fbs[display_id] = framebuffer.clone();

                    if *vt == self.active_vt {
                        self.adapter
                            .update_plane(display_id, framebuffer, payload.damage);
                    }

                    Ok(size_of::<ipc::UpdatePlane>())
                }
                _ => return Err(Error::new(EINVAL)),
            },
        }
    }

    fn mmap_prep(
        &mut self,
        id: usize,
        offset: u64,
        _size: usize,
        _flags: MapFlags,
        _ctx: &CallerCtx,
    ) -> syscall::Result<usize> {
        // log::trace!("KSMSG MMAP {} {:?} {} {}", id, _flags, _offset, _size);
        let (framebuffer, offset) = match self.handles.get(&id).ok_or(Error::new(EINVAL))? {
            Handle::V1Screen { vt, screen } => (&self.vts[vt].display_fbs[*screen], offset),
            Handle::V2 {
                vt: _,
                next_id: _,
                fbs,
            } => (
                fbs.get(&(offset as usize / MAP_FAKE_OFFSET_MULTIPLIER))
                    .ok_or(Error::new(EINVAL))
                    .unwrap(),
                offset & (MAP_FAKE_OFFSET_MULTIPLIER as u64 - 1),
            ),
        };
        let ptr = T::map_dumb_framebuffer(&mut self.adapter, framebuffer);
        Ok(unsafe { ptr.add(offset as usize) } as usize)
    }
}

impl<T: GraphicsAdapter> GraphicsScheme<T> {
    fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
