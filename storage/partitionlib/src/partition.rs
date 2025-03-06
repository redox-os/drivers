use super::Result;
pub use gpt::disk::LogicalBlockSize;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use uuid::Uuid;

/// A union of the MBR and GPT partition entry
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Partition {
    /// The starting logical block number
    pub start_lba: u64,
    /// The size of the partition in sectors
    pub size: u64,
    pub flags: Option<u64>,
    pub name: Option<String>,
    pub uuid: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PartitionTableKind {
    Mbr,
    Gpt,
}
impl Default for PartitionTableKind {
    fn default() -> Self {
        Self::Gpt
    }
}
impl PartitionTableKind {
    pub fn is_mbr(self) -> bool {
        self == Self::Mbr
    }
    pub fn is_gpt(self) -> bool {
        self == Self::Gpt
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PartitionTable {
    pub partitions: Vec<Partition>,
    pub kind: PartitionTableKind,
}

pub fn get_partitions_from_file<P: AsRef<Path>>(
    path: P,
    sector_size: LogicalBlockSize,
) -> Result<Option<PartitionTable>> {
    let mut file = File::open(path)?;
    get_partitions(&mut file, sector_size)
}
fn get_gpt_partitions<D: Read + Seek>(
    device: &mut D,
    sector_size: LogicalBlockSize,
) -> Result<PartitionTable> {
    let header = match gpt::header::read_header_from_arbitrary_device(device, sector_size) {
        Ok(res) => res,
        Err(err) => return Err(err),
    };
    Ok(PartitionTable {
        partitions: gpt::partition::file_read_partitions(device, &header, sector_size).map(
            |btree| {
                btree
                    .into_iter()
                    .map(|(_, part)| Partition {
                        flags: Some(part.flags),
                        size: part.last_lba - part.first_lba + 1,
                        name: Some(part.name.clone()),
                        uuid: Some(part.part_guid),
                        start_lba: part.first_lba,
                    })
                    .collect()
            },
        )?,
        kind: PartitionTableKind::Gpt,
    })
}
fn get_mbr_partitions<D: Read + Seek>(device: &mut D) -> Result<Option<PartitionTable>> {
    let header = match crate::mbr::read_header(device) {
        Ok(h) => h,
        Err(crate::mbr::Error::ParsingError(_)) => return Ok(None),
        Err(crate::mbr::Error::IoError(ioerr)) => return Err(ioerr),
    };
    Ok(Some(PartitionTable {
        kind: PartitionTableKind::Mbr,
        partitions: header
            .partitions()
            .iter()
            .copied()
            .filter(crate::mbr::Entry::is_valid)
            .map(|partition: crate::mbr::Entry| Partition {
                name: None,
                uuid: None,  // TODO: Some kind of one-way conversion should be possible
                flags: None, // TODO
                size: partition.len.into(),
                start_lba: partition.rel_sector.into(),
            })
            .collect(),
    }))
}
pub fn get_partitions<D: Read + Seek>(
    device: &mut D,
    sector_size: LogicalBlockSize,
) -> Result<Option<PartitionTable>> {
    get_gpt_partitions(device, sector_size)
        .map(Some)
        .or_else(|_| get_mbr_partitions(device))
}

impl Partition {
    pub fn to_offset(&self, sector_size: LogicalBlockSize) -> u64 {
        let blksize: u64 = sector_size.into();
        self.start_lba * blksize
    }
}
