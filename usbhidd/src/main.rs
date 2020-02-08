use std::convert::TryInto;
use std::env;

use bitflags::bitflags;
use ux::{u2, u4};
use xhcid_interface::{DevDesc, XhciClientHandle};

/*#[repr(u8)]
enum Protocol {

}*/

bitflags! {
    pub struct MainItemFlags: u32 {
        const CONSTANT = 1 << 0;
        const VARIABLE = 1 << 1;
        const RELATIVE = 1 << 2;
        const WRAP = 1 << 3;
        const NONLINEAR = 1 << 4;
        const NO_PREFERRED_STATE = 1 << 5;
        const NULL_STATE = 1 << 6;
        const VOLATILE = 1 << 7;
        const BUFFERED_BYTES = 1 << 8;
    }
}
#[repr(u8)]
pub enum MainCollectionFlags {
    Physical = 0,
    Application,
    Logical,
    Report,
    NamedArray,
    UsageSwitch,
    UsageModifier,
}

const REPORT_DESC_TY: u8 = 34;

#[derive(Debug)]
enum MainItem {
    Input(u32),
    Output(u32),
    Feature(u32),
    Collection(u8),
    EndOfCollection,
}
#[derive(Debug)]
enum GlobalItem {
    UsagePage(u32),
    LogicalMinimum(u32),
    LogicalMaximum(u32),
    PhysicalMinimum(u32),
    PhysicalMaximum(u32),
    UnitExponent(u32),
    Unit(u32),
    ReportSize(u32),
    RepordId(u32),
    ReportCount(u32),
    Push(u32),
    Pop(u32),
}
#[derive(Debug)]
enum LocalItem {
    Usage(u32),
    UsageMinimum(u32),
    UsageMaximum(u32),
    DesignatorIndex(u32),
    DesignatorMinimum(u32),
    DesignatorMaximum(u32),
    StringIndex(u32),
    StringMinimum(u32),
    StringMaximum(u32),
    Delimeter(u32),
}
#[derive(Debug)]
enum ReportItem {
    Main(MainItem),
    Global(GlobalItem),
    Local(LocalItem),
}
impl From<MainItem> for ReportItem {
    fn from(main: MainItem) -> Self {
        Self::Main(main)
    }
}
impl From<GlobalItem> for ReportItem {
    fn from(main: GlobalItem) -> Self {
        Self::Global(main)
    }
}
impl From<LocalItem> for ReportItem {
    fn from(main: LocalItem) -> Self {
        Self::Local(main)
    }
}

impl ReportItem {
    fn size(size: u2) -> u8 {
        match u8::from(size) {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 4,
            _ => unreachable!(),
        }
    }
    fn parse_short(size: u2, ty: u2, tag: u4, bytes: &[u8]) -> Option<(Self, usize)> {
        Some(match (u8::from(tag), u8::from(ty)) {
            (tag, 0b00) => {
                let real_size = Self::size(size) as usize;
                let mut value_bytes = [0u8; 4];
                if real_size > 0 {
                    value_bytes[..real_size].copy_from_slice(&bytes[..real_size])
                };
                let value = u32::from_le_bytes(value_bytes);

                match tag {
                    0b1000 => (MainItem::Input(value).into(), 1 + real_size),
                    0b1001 => (MainItem::Output(value).into(), 1 + real_size),
                    0b1011 => (MainItem::Feature(value).into(), 1 + real_size),
                    0b1010 => (MainItem::Collection(bytes[0]).into(), 2),
                    0b1100 => (MainItem::EndOfCollection.into(), 1 + real_size),
                    _ => return None,
                }
            }
            (tag, 0b01) => {
                let real_size = Self::size(size) as usize;
                let mut value_bytes = [0u8; 4];
                if real_size > 0 {
                    value_bytes[..real_size].copy_from_slice(&bytes[..real_size])
                };
                let value = u32::from_le_bytes(value_bytes);
                match tag {
                    0b0000 => (GlobalItem::UsagePage(value).into(), 1 + real_size),
                    0b0001 => (GlobalItem::LogicalMinimum(value).into(), 1 + real_size),
                    0b0010 => (GlobalItem::LogicalMaximum(value).into(), 1 + real_size),
                    0b0011 => (GlobalItem::PhysicalMinimum(value).into(), 1 + real_size),
                    0b0100 => (GlobalItem::PhysicalMaximum(value).into(), 1 + real_size),
                    0b0101 => (GlobalItem::UnitExponent(value).into(), 1 + real_size),
                    0b0110 => (GlobalItem::Unit(value).into(), 1 + real_size),
                    0b0111 => (GlobalItem::ReportSize(value).into(), 1 + real_size),
                    0b1000 => (GlobalItem::RepordId(value).into(), 1 + real_size),
                    0b1001 => (GlobalItem::ReportCount(value).into(), 1 + real_size),
                    0b1010 => (GlobalItem::Push(value).into(), 1 + real_size),
                    0b1011 => (GlobalItem::Pop(value).into(), 1 + real_size),
                    _ => return None,
                }
            }
            (tag, 0b10) => {
                let real_size = Self::size(size) as usize;
                let mut value_bytes = [0u8; 4];
                if real_size > 0 {
                    value_bytes[..real_size].copy_from_slice(&bytes[..real_size])
                };
                let value = u32::from_le_bytes(value_bytes);
                match tag {
                    0b0000 => (LocalItem::Usage(value).into(), 1 + real_size),
                    0b0001 => (LocalItem::UsageMinimum(value).into(), 1 + real_size),
                    0b0010 => (LocalItem::UsageMaximum(value).into(), 1 + real_size),
                    0b0011 => (LocalItem::DesignatorIndex(value).into(), 1 + real_size),
                    0b0100 => (LocalItem::DesignatorMinimum(value).into(), 1 + real_size),
                    0b0101 => (LocalItem::DesignatorMaximum(value).into(), 1 + real_size),
                    0b0111 => (LocalItem::StringIndex(value).into(), 1 + real_size),
                    0b1000 => (LocalItem::StringMinimum(value).into(), 1 + real_size),
                    0b1001 => (LocalItem::StringMaximum(value).into(), 1 + real_size),
                    0b1010 => (LocalItem::Delimeter(value).into(), 1 + real_size),
                    _ => return None,
                }
            }
            (_, 0b11) => panic!("Calling parse_short but with long item"),
            _ => unreachable!(),
        })
    }
    fn parse_long(size: u8, long_tag: u8, bytes: &[u8]) -> (Self, usize) {
        todo!()
    }
}

struct ReportIter<'a> {
    desc: &'a [u8],
    offset: usize,
}
impl<'a> ReportIter<'a> {
    fn new(desc: &'a [u8]) -> Self {
        Self { desc, offset: 0 }
    }
}
impl<'a> Iterator for ReportIter<'a> {
    type Item = ReportItem;

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.desc[self.offset];
        let size = first & 0b11;
        let ty = first & 0b1100 >> 2;
        let tag = first & 0b11110000 >> 4;

        if size == 0b10 && ty == 3 && tag == 0b1111 {
            // Long Item
            let size = self.desc[self.offset + 1];
            let long_tag = self.desc[self.offset + 2];
            let data = &self.desc[self.offset + 2..self.offset + 2 + size as usize];

            let (item, len) = ReportItem::parse_long(size, long_tag, data);
            self.offset += len;
            Some(item)
        } else {
            // Short Item

            // Although there is a 2-bit size field, the size doesn't have to be the actual size of the data.
            let data = &self.desc[self.offset + 1..];

            let (item, len) =
                ReportItem::parse_short(u2::new(size), u2::new(ty), u4::new(tag), data)?;
            self.offset += len;
            Some(item)
        }
    }
}

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbhidd <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("Expected integer as input of port");
    let protocol = args.next().expect(USAGE);

    println!(
        "USB HID driver spawned with scheme `{}`, port {}, protocol {}",
        scheme, port, protocol
    );

    let handle = XhciClientHandle::new(scheme, port);
    let dev_desc: DevDesc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");
    let hid_desc = dev_desc.config_descs[0].interface_descs[0].hid_descs[0];

    // TODO: Currently it's assumed that config 0 and interface 0 are used.

    let interface_num = 0;
    let report_desc_len = hid_desc.desc_len;
    assert_eq!(hid_desc.desc_ty, REPORT_DESC_TY);

    let report_desc_bytes: Vec<u8> = handle
        .get_class_descriptor(
            u16::from(REPORT_DESC_TY) << 8,
            interface_num,
            report_desc_len,
        )
        .expect("Failed to retrieve report descriptor");
    use std::io::Write as _;
    std::io::stdout()
        .write_all(base64::encode(&report_desc_bytes).as_bytes())
        .unwrap();
    let iterator = ReportIter::new(&report_desc_bytes);

    for item in iterator {
        println!("HID ITEM {:?}", item);
    }
}
