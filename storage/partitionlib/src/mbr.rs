use scroll::{Pread, Pwrite};
use std::io;
use std::io::prelude::*;

#[derive(Clone, Copy, Debug, Pread, Pwrite)]
pub struct Entry {
    pub drive_attrs: u8,
    pub start_head: u8,
    pub start_cs: u16,
    pub sys_id: u8,
    pub end_head: u8,
    pub end_cs: u16,
    pub rel_sector: u32,
    pub len: u32,
}

#[derive(Pread, Pwrite)]
pub struct Header {
    pub bootstrap: [u8; 446],
    pub first_entry: Entry,
    pub second_entry: Entry,
    pub third_entry: Entry,
    pub fourth_entry: Entry,
    pub last_signature: u16, // 0xAA55
}

#[derive(Debug)]
pub enum Error {
    IoError(io::Error),
    ParsingError(scroll::Error),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::IoError(err)
    }
}
impl From<scroll::Error> for Error {
    fn from(err: scroll::Error) -> Self {
        Self::ParsingError(err)
    }
}

pub fn read_header<D: Read + Seek>(device: &mut D) -> Result<Header, Error> {
    device.seek(io::SeekFrom::Start(0))?;

    let mut bytes = [0u8; 512];
    device.read_exact(&mut bytes)?;

    let header: Header = bytes.pread_with(0, scroll::LE)?;

    if header.last_signature != 0xAA55 {
        return Err(scroll::Error::BadInput {
            size: 2,
            msg: "no 0xAA55 signature",
        }
        .into());
    }

    Ok(header)
}

impl Header {
    pub fn partitions(&self) -> [Entry; 4] {
        [
            self.first_entry,
            self.second_entry,
            self.third_entry,
            self.fourth_entry,
        ]
    }
}
impl Entry {
    pub fn is_valid(&self) -> bool {
        (self.drive_attrs == 0 || self.drive_attrs == 0x80) && self.len != 0
    }
}
