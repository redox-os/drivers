pub use gpt::disk::LogicalBlockSize;
use std::io::{self, Read, Seek};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionTableKind {
    Mbr,
    Gpt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionTable {
    pub partitions: Vec<Partition>,
    pub kind: PartitionTableKind,
}

fn get_gpt_partitions<D: Read + Seek>(
    device: &mut D,
    sector_size: LogicalBlockSize,
) -> io::Result<PartitionTable> {
    let header = gpt::header::read_header_from_arbitrary_device(device, sector_size)?;
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
fn get_mbr_partitions<D: Read + Seek>(device: &mut D) -> io::Result<Option<PartitionTable>> {
    let Some(header) = crate::mbr::read_header(device)? else {
        return Ok(None);
    };
    Ok(Some(PartitionTable {
        kind: PartitionTableKind::Mbr,
        partitions: header
            .partitions()
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
) -> io::Result<Option<PartitionTable>> {
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
