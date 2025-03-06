extern crate partitionlib;

use partitionlib::{get_partitions_from_file, LogicalBlockSize, Partition};

#[test]
fn part_is_gpt() {
    assert!(dbg!(
        get_partitions_from_file("./resources/disk.img", LogicalBlockSize::Lb512)
            .unwrap()
            .unwrap()
    )
    .kind
    .is_gpt());
}

#[test]
fn part_is_mbr() {
    assert!(
        get_partitions_from_file("./resources/disk_mbr.img", LogicalBlockSize::Lb512)
            .unwrap()
            .unwrap()
            .kind
            .is_mbr()
    );
}

// NOTE: The following tests rely on outside resource files being correct.
#[test]
fn gpt() {
    let table = get_partitions_from_file("./resources/disk.img", LogicalBlockSize::Lb512).unwrap().unwrap();
    assert_eq!(&table.partitions, &[
        Partition {
            flags: Some(0),
            name: Some("bug".to_owned()),
            uuid: Some(uuid::Uuid::parse_str("b665fba9-74d5-4069-a6b9-5ba3a164fdfe").unwrap()), // Microsoft basic data
            size: 957,
            start_lba: 34,
        }
    ]);
}

#[test]
fn mbr() {
    let table = get_partitions_from_file("./resources/disk_mbr.img", LogicalBlockSize::Lb512).unwrap().unwrap();
    assert_eq!(&table.partitions, &[
        Partition {
            flags: None,
            name: None,
            uuid: None,
            size: 3,
            start_lba: 1,
        }
    ]);
}
