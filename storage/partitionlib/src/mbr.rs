use scroll::{Pread, Pwrite};
use std::io::{self, Read, Seek};

#[derive(Clone, Copy, Debug, Pread, Pwrite)]
pub(crate) struct Entry {
    pub(crate) drive_attrs: u8,
    pub(crate) start_head: u8,
    pub(crate) start_cs: u16,
    pub(crate) sys_id: u8,
    pub(crate) end_head: u8,
    pub(crate) end_cs: u16,
    pub(crate) rel_sector: u32,
    pub(crate) len: u32,
}

#[derive(Pread, Pwrite)]
pub(crate) struct Header {
    pub(crate) bootstrap: [u8; 446],
    pub(crate) first_entry: Entry,
    pub(crate) second_entry: Entry,
    pub(crate) third_entry: Entry,
    pub(crate) fourth_entry: Entry,
    pub(crate) last_signature: u16, // 0xAA55
}

pub(crate) fn read_header<D: Read + Seek>(device: &mut D) -> io::Result<Option<Header>> {
    device.seek(io::SeekFrom::Start(0))?;

    let mut bytes = [0u8; 512];
    device.read_exact(&mut bytes)?;

    let header: Header = bytes.pread_with(0, scroll::LE).unwrap();

    if header.last_signature != 0xAA55 {
        return Ok(None);
    }

    Ok(Some(header))
}

impl Header {
    pub(crate) fn partitions(&self) -> impl Iterator<Item = Entry> {
        [
            self.first_entry,
            self.second_entry,
            self.third_entry,
            self.fourth_entry,
        ]
        .into_iter()
        .filter(Entry::is_valid)
    }
}
impl Entry {
    fn is_valid(&self) -> bool {
        (self.drive_attrs == 0 || self.drive_attrs == 0x80) && self.len != 0
    }
}
