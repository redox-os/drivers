#![feature(iter_next_chunk)]

use std::fs::File;
use std::io::{Error, Read, Write};

unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    ::core::slice::from_raw_parts((p as *const T) as *const u8, ::core::mem::size_of::<T>())
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct VtActivate {
    pub vt: usize,
}

pub struct Handle(File);

impl Handle {
    pub fn new<S: Into<String>>(device_name: S) -> Result<Self, Error> {
        let path = format!("/scheme/input/handle/display/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    // The return value is the display identifier. It will be used to uniquely
    // identify the display on activation events.
    pub fn register(&mut self) -> Result<usize, Error> {
        self.0.read(&mut [])
    }

    pub fn activate(&mut self, vt: usize) -> Result<usize, Error> {
        let cmd = VtActivate { vt };
        self.0.write(unsafe { any_as_u8_slice(&cmd) })
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u8)]
enum CmdTy {
    Unknown = 0,

    Activate,
    Deactivate,
    Resize,
}

impl From<u8> for CmdTy {
    fn from(value: u8) -> Self {
        match value {
            1 => CmdTy::Activate,
            2 => CmdTy::Deactivate,
            3 => CmdTy::Resize,
            _ => CmdTy::Unknown,
        }
    }
}

#[derive(Debug)]
pub enum Cmd {
    // TODO(andypython): #VT should really need to be a `u8`.
    Activate {
        vt: usize,
    },

    Deactivate(usize /* #VT */),
    Resize {
        // TODO(andypython): do we really need to pass the VT here?
        vt: usize,

        width: u32,
        height: u32,
        stride: u32,
    },
}

impl Cmd {
    fn ty(&self) -> CmdTy {
        match self {
            Cmd::Activate { .. } => CmdTy::Activate,
            Cmd::Deactivate(_) => CmdTy::Deactivate,
            Cmd::Resize { .. } => CmdTy::Resize,
        }
    }
}

pub fn send_comand(file: &mut File, command: Cmd) -> Result<(), libredox::error::Error> {
    use std::os::fd::AsRawFd;

    let mut result = vec![];
    result.push(command.ty() as u8);

    match command {
        Cmd::Activate { vt } => {
            let cmd = VtActivate { vt };
            let bytes = unsafe { any_as_u8_slice(&cmd) };

            result.extend_from_slice(bytes);
        }

        Cmd::Deactivate(vt) => result.extend_from_slice(&vt.to_le_bytes()),
        Cmd::Resize {
            vt,
            width,
            height,
            stride,
        } => {
            result.extend_from_slice(&vt.to_le_bytes());
            result.extend(width.to_le_bytes());
            result.extend(height.to_le_bytes());
            result.extend(stride.to_le_bytes());
        }
    };

    let written = libredox::call::write(file.as_raw_fd() as usize, &result)?;

    // XXX: Ensure all of the data is written.
    assert_eq!(written, result.len());
    Ok(())
}

pub fn parse_command(buffer: &[u8]) -> Option<Cmd> {
    const U32_SIZE: usize = core::mem::size_of::<u32>();
    const USIZE_SIZE: usize = core::mem::size_of::<usize>();

    let mut parser = buffer.iter().cloned();

    let command = CmdTy::from(parser.next()?);
    let vt = usize::from_le_bytes(parser.next_chunk::<USIZE_SIZE>().ok()?);

    match command {
        CmdTy::Activate => {
            let cmd = unsafe { &*buffer.as_ptr().offset(1).cast::<VtActivate>() };
            Some(Cmd::Activate { vt: cmd.vt })
        }

        CmdTy::Deactivate => Some(Cmd::Deactivate(vt)),
        CmdTy::Resize => {
            let width = parser.next_chunk::<U32_SIZE>().ok()?;
            let height = parser.next_chunk::<U32_SIZE>().ok()?;
            let stride = parser.next_chunk::<U32_SIZE>().ok()?;

            Some(Cmd::Resize {
                vt,
                width: u32::from_le_bytes(width),
                height: u32::from_le_bytes(height),
                stride: u32::from_le_bytes(stride),
            })
        }

        CmdTy::Unknown => None,
    }
}

#[repr(packed)]
pub struct Damage {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
