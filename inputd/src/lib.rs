use std::fs::File;
use std::io::{Error, Read};

pub struct Handle(File);

impl Handle {
    pub fn new<S: Into<String>>(device_name: S) -> Result<Self, Error> {
        let path = format!("input:handle/{}", device_name.into());
        Ok(Self(File::open(path)?))
    }

    // The return value is the display identifier. It will be used to uniquely
    // identify the display on activation events.
    pub fn register(&mut self) -> Result<usize, Error> {
        Ok(dbg!(self.0.read(&mut [])?))
    }
}

#[repr(packed)]
pub struct Damage {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
