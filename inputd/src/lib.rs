use std::fs::File;
use std::io::{Error, Read};
use std::mem;

pub struct Handle(File);

impl Handle {
    pub fn new<S: Into<String>>(device_name: S) -> Result<Self, Error> {
        let path = format!("input:handle/display/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    // The return value is the display identifier. It will be used to uniquely
    // identify the display on activation events.
    pub fn register(&mut self) -> Result<usize, Error> {
        Ok(dbg!(self.0.read(&mut [])?))
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum CommandTy {
    Unknown = 0,

    Activate,
    Deactivate,
}

#[repr(C)]
pub struct Command {
    pub ty: CommandTy,
    pub value: usize,
}

impl Command {
    pub fn new(ty: CommandTy, value: usize) -> Self {
        Self { ty, value }
    }

    /// ## Panics
    /// This function panics if the buffer is not `sizeof::<Command>()` bytes wide.
    pub fn parse<'a>(buffer: &'a [u8]) -> &'a Command {
        assert_eq!(buffer.len(), core::mem::size_of::<Command>());
        unsafe { &*(buffer.as_ptr() as *const Command) }
    }

    pub fn into_bytes(self) -> [u8; mem::size_of::<Command>()] {
        unsafe { core::mem::transmute(self) }
    }
}

#[repr(packed)]
pub struct Damage {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
