use std::fs::File;

use partitionlib::{
    get_partitions, LogicalBlockSize, Partition, PartitionTable, PartitionTableKind,
};

fn get_partitions_from_file(path: &str) -> PartitionTable {
    let mut file = File::open(path).unwrap();
    get_partitions(&mut file, LogicalBlockSize::Lb512)
        .unwrap()
        .unwrap()
}

// NOTE: The following tests rely on outside resource files being correct.
#[test]
fn gpt() {
    let table = get_partitions_from_file("./resources/disk.img");
    assert_eq!(table.kind, PartitionTableKind::Gpt);
    assert_eq!(
        &table.partitions,
        &[Partition {
            flags: Some(0),
            name: Some("bug".to_owned()),
            uuid: Some(uuid::Uuid::parse_str("b665fba9-74d5-4069-a6b9-5ba3a164fdfe").unwrap()), // Microsoft basic data
            size: 957,
            start_lba: 34,
        }]
    );
}

#[test]
fn mbr() {
    let table = get_partitions_from_file("./resources/disk_mbr.img");
    assert_eq!(table.kind, PartitionTableKind::Mbr);
    assert_eq!(
        &table.partitions,
        &[Partition {
            flags: None,
            name: None,
            uuid: None,
            size: 3,
            start_lba: 1,
        }]
    );
}
