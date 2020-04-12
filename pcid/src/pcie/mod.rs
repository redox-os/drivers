use std::{fs, io, mem, slice};
use smallvec::SmallVec;

pub const MCFG_NAME: [u8; 4] = *b"MCFG";

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct Mcfg {
    // base sdt fields
    name: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: [u8; 4],
    _rsvd: [u8; 8],

    base_addrs: [PcieAlloc; 0],
}
unsafe impl plain::Plain for Mcfg {}

/// The "Memory Mapped Enhanced Configuration Space Base Address Allocation Structure" (yes, it's
/// called that).

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct PcieAlloc {
    pub base_addr: u64,
    pub seg_group_num: u16,
    pub start_bus: u8,
    pub end_bus: u8,
    _rsvd: [u8; 4],
}
unsafe impl plain::Plain for PcieAlloc {}

impl Mcfg {
    pub fn base_addr_structs(&self) -> &[PcieAlloc] {
        let total_length = mem::size_of::<Self>();
        let len = total_length - 44;
        // safe because the length cannot be changed arbitrarily
        unsafe { slice::from_raw_parts(&self.base_addrs as *const PcieAlloc, len / mem::size_of::<PcieAlloc>()) }
    }
}

pub struct Mcfgs {
    tables: SmallVec<[Vec<u8>; 2]>,
}
impl Mcfgs {
    pub fn tables<'a>(&'a self) -> impl Iterator<Item = &'a Mcfg> + 'a {
        self.tables.iter().filter_map(|bytes| {
            let mcfg = plain::from_bytes::<Mcfg>(bytes).ok()?;
            if mcfg.length as usize > bytes.len() {
                return None;
            }
            Some(mcfg)
        })
    }

    pub fn fetch() -> io::Result<Self> {
        let table_dir = fs::read_dir("acpi:tables")?;

        let tables = table_dir.map(|table_direntry| -> io::Result<Option<_>> {
            let table_direntry = table_direntry?;
            let table_path = table_direntry.path();

            let table_filename = match table_path.file_name() {
                Some(n) => n.to_str().ok_or(io::Error::new(io::ErrorKind::InvalidData, "Non-UTF-8 ACPI table filename"))?,
                None => return Ok(None),
            };

            if table_filename.starts_with("MCFG") {
                Ok(Some(fs::read(table_path)?))
            } else {
                Ok(None)
            }
        }).filter_map(|result_option| result_option.transpose()).collect::<Result<SmallVec<_>, _>>()?;

        Ok(Self {
            tables,
        })
    }
    pub fn at_bus(&self, bus: u8) -> Option<(&Mcfg, &PcieAlloc)> {
        self.tables().find_map(|table| {
            Some((table, table.base_addr_structs().iter().find(|addr_struct| {
                (addr_struct.start_bus..addr_struct.end_bus).contains(&bus)
            })?))
        })
    }
}
