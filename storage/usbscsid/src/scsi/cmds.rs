use super::opcodes::Opcode;
use std::convert::TryInto;
use std::{fmt, mem, slice};

#[repr(C, packed)]
pub struct Inquiry {
    pub opcode: u8,
    /// bits 7:2 are reserved, bit 1 (CMDDT) is obsolete, bit 0 is EVPD
    pub evpd: u8,
    pub page_code: u8,
    /// big endian
    pub alloc_len: u16,
    pub control: u8,
}
unsafe impl plain::Plain for Inquiry {}

impl Inquiry {
    pub const fn new(evpd: bool, page_code: u8, alloc_len: u16, control: u8) -> Self {
        Self {
            opcode: Opcode::Inquiry as u8,
            evpd: evpd as u8,
            page_code,
            alloc_len: u16::to_be(alloc_len),
            control,
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct StandardInquiryData {
    /// Peripheral device type (bits 4:0), and peripheral device qualifier (bits 7:5).
    pub a: u8,
    /// Removable media bit (bit 7, bits 6:0 are reserved).
    pub rmb: u8,
    /// Version of the SCSI command set.
    pub version: u8,
    pub b: u8,
    pub additional_len: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub t10_vendor_info: u64,
    pub product_ident: [u8; 16],
    pub product_rev_label: u32,
    pub driver_serial_no: [u8; 8],
    pub vendor_uniq: [u8; 12],
    _rsvd1: [u8; 2],
    pub version_descs: [u16; 8],
    _rsvd2: [u8; 22],
}
unsafe impl plain::Plain for StandardInquiryData {}
impl StandardInquiryData {
    pub const fn periph_dev_ty(&self) -> u8 {
        self.a & 0x1F
    }
    pub const fn periph_dev_qual(&self) -> u8 {
        (self.a & 0xE0) >> 5
    }
    pub const fn version(&self) -> u8 {
        self.version
    }
}

#[repr(u8)]
pub enum PeriphDeviceType {
    DirectAccess,
    SeqAccess,
    // there are more
}
#[repr(u8)]
pub enum InquiryVersion {
    NoConformance,
    Spc,
    Spc2,
    Spc3,
    Spc4,
    Spc5,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct RequestSense {
    pub opcode: u8,
    pub desc: u8, // bits 7:1 reserved
    _rsvd: u16,
    pub alloc_len: u8,
    pub control: u8,
}
unsafe impl plain::Plain for RequestSense {}

impl RequestSense {
    pub const MINIMAL_ALLOC_LEN: u8 = 252;

    pub const fn new(desc: bool, alloc_len: u8, control: u8) -> Self {
        Self {
            opcode: Opcode::RequestSense as u8,
            desc: desc as u8,
            _rsvd: 0,
            alloc_len,
            control,
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct FixedFormatSenseData {
    pub a: u8,
    _obsolete: u8,
    pub b: u8,
    pub info: u32,
    pub add_sense_len: u8,
    pub command_specific_info: u32,
    pub add_sense_code: u8,
    pub add_sense_code_qual: u8,
    pub field_replacable_unit_code: u8,
    pub sense_key_specific: [u8; 3], // big endian
    pub add_sense_bytes: [u8; 0],
}
unsafe impl plain::Plain for FixedFormatSenseData {}

impl FixedFormatSenseData {
    pub const fn additional_len(&self) -> u16 {
        self.add_sense_len as u16 + 7
    }
    pub unsafe fn add_sense_bytes(&self) -> &[u8] {
        slice::from_raw_parts(
            &self.add_sense_len as *const u8,
            self.add_sense_len as usize - 18,
        )
    }
    pub fn sense_key(&self) -> SenseKey {
        let sense_key_raw = self.b & 0b1111;
        // Safe because all possible values (0-15) are used by the enum.
        unsafe { mem::transmute(sense_key_raw) }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SenseKey {
    NoSense = 0x00,
    RecoveredError = 0x01,
    NotReady = 0x02,
    MediumError = 0x03,
    HardwareError = 0x04,
    IllegalRequest = 0x05,
    UnitAttention = 0x06,
    DataProtect = 0x07,
    BlankCheck = 0x08,
    VendorSpecific = 0x09,
    CopyAborted = 0x0A,
    AbortedCommand = 0x0B,
    Reserved = 0x0C,
    VolumeOverflow = 0x0D,
    Miscompare = 0x0E,
    Completed = 0x0F,
}
impl Default for SenseKey {
    fn default() -> Self {
        Self::NoSense
    }
}

pub const ADD_SENSE_CODE05_INVAL_CDB_FIELD: u8 = 0x24;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Read16 {
    pub opcode: u8,
    pub a: u8,
    pub lba: u64,
    pub transfer_len: u32,
    pub b: u8,
    pub control: u8,
}
unsafe impl plain::Plain for Read16 {}

impl Read16 {
    pub const fn new(lba: u64, transfer_len: u32, control: u8) -> Self {
        // TODO: RDPROTECT, DPO, FUA, RARC
        // TODO: DLD
        // TODO: Group number
        Self {
            opcode: Opcode::Read16 as u8,
            a: 0,
            lba: u64::to_be(lba),
            transfer_len: u32::to_be(transfer_len),
            b: 0,
            control,
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Write16 {
    pub opcode: u8,
    pub a: u8,
    pub lba: u64, // big endian
    pub transfer_len: u32,
    pub b: u8,
    pub control: u8,
}
unsafe impl plain::Plain for Write16 {}

impl Write16 {
    pub const fn new(lba: u64, transfer_len: u32, control: u8) -> Self {
        Self {
            // TODO
            opcode: Opcode::Write16 as u8,
            a: 0,
            lba,
            transfer_len,
            b: 0,
            control,
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModeSense6 {
    pub opcode: u8,
    pub a: u8,
    pub b: u8,
    pub subpage_code: u8,
    pub alloc_len: u8,
    pub control: u8,
}
unsafe impl plain::Plain for ModeSense6 {}

impl ModeSense6 {
    pub const fn new(
        dbd: bool,
        page_code: u8,
        pc: u8,
        subpage_code: u8,
        alloc_len: u8,
        control: u8,
    ) -> Self {
        Self {
            opcode: Opcode::ModeSense6 as u8,
            a: (dbd as u8) << 3,
            b: page_code | (pc << 6),
            subpage_code,
            alloc_len,
            control,
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModeSense10 {
    pub opcode: u8,
    pub a: u8,
    pub b: u8,
    pub subpage_code: u8,
    pub _rsvd: [u8; 3],
    pub alloc_len: u16,
    pub control: u8,
}
unsafe impl plain::Plain for ModeSense10 {}

impl ModeSense10 {
    pub const fn new(
        dbd: bool,
        llbaa: bool,
        page_code: u8,
        pc: ModePageControl,
        subpage_code: u8,
        alloc_len: u16,
        control: u8,
    ) -> Self {
        Self {
            opcode: Opcode::ModeSense10 as u8,
            a: ((dbd as u8) << 3) | ((llbaa as u8) << 4),
            b: page_code | ((pc as u8) << 6),
            subpage_code,
            _rsvd: [0u8; 3],
            alloc_len: u16::from_be(alloc_len),
            control,
        }
    }
    pub const fn get_block_desc(alloc_len: u16, control: u8) -> Self {
        Self::new(
            false,
            true,
            0x3F,
            ModePageControl::CurrentValues,
            0x00,
            alloc_len,
            control,
        )
    }
}

#[repr(u8)]
pub enum ModePageControl {
    CurrentValues,
    ChangeableChanges,
    DefaultValues,
    SavedValue,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ShortLbaModeParamBlkDesc {
    pub block_count: u32,
    _rsvd: u8,
    pub logical_block_len: [u8; 3],
}
unsafe impl plain::Plain for ShortLbaModeParamBlkDesc {}

impl ShortLbaModeParamBlkDesc {
    pub const fn block_count(&self) -> u32 {
        u32::from_be(self.block_count)
    }
    pub const fn logical_block_len(&self) -> u32 {
        u24_be_to_u32(self.logical_block_len)
    }
}
impl fmt::Debug for ShortLbaModeParamBlkDesc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ShortLbaModeParamBlkDesc")
            .field("block_count", &self.block_count())
            .field("logical_block_len", &self.logical_block_len())
            .finish()
    }
}

const fn u24_be_to_u32(u24: [u8; 3]) -> u32 {
    ((u24[0] as u32) << 16) | ((u24[1] as u32) << 8) | (u24[2] as u32)
}

/// From SPC-3, when LONGLBA is not set, and the peripheral device type of the INQUIRY data indicates that the device is not a direct access device. Otherwise, `ShortLbaModeParamBlkDesc` is used instead.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct GeneralModeParamBlkDesc {
    pub density_code: u8,
    pub block_count: [u8; 3],
    _rsvd: u8,
    pub block_length: [u8; 3],
}
unsafe impl plain::Plain for GeneralModeParamBlkDesc {}

impl GeneralModeParamBlkDesc {
    pub fn block_count(&self) -> u32 {
        u24_be_to_u32(self.block_count)
    }
    pub fn logical_block_len(&self) -> u32 {
        u24_be_to_u32(self.block_length)
    }
}

impl fmt::Debug for GeneralModeParamBlkDesc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("GeneralModeParamBlkDesc")
            .field("density_code", &self.density_code)
            .field("block_count", &u24_be_to_u32(self.block_count))
            .field("block_length", &u24_be_to_u32(self.block_length))
            .finish()
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LongLbaModeParamBlkDesc {
    pub block_count: u64,
    _rsvd: u32,
    pub logical_block_len: u32,
}
unsafe impl plain::Plain for LongLbaModeParamBlkDesc {}

impl LongLbaModeParamBlkDesc {
    pub const fn block_count(&self) -> u64 {
        u64::from_be(self.block_count)
    }
    pub const fn logical_block_len(&self) -> u32 {
        u32::from_be(self.logical_block_len)
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModeParamHeader6 {
    pub mode_data_len: u8,
    pub medium_ty: u8,
    pub a: u8,
    pub block_desc_len: u8,
}
unsafe impl plain::Plain for ModeParamHeader6 {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModeParamHeader10 {
    pub mode_data_len: u16,
    pub medium_ty: u8,
    pub a: u8,
    pub b: u8,
    _rsvd: u8,
    pub block_desc_len: u16,
}
unsafe impl plain::Plain for ModeParamHeader10 {}
impl ModeParamHeader10 {
    pub const fn mode_data_len(&self) -> u16 {
        u16::from_be(self.mode_data_len)
    }
    pub const fn block_desc_len(&self) -> u16 {
        u16::from_be(self.block_desc_len)
    }
    pub const fn longlba(&self) -> bool {
        (self.b & 0x01) != 0
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ReadCapacity10 {
    pub opcode: u8,
    _rsvd1: u8,
    obsolete_lba: u32,
    _rsvd2: [u8; 3],
    pub control: u8,
}
unsafe impl plain::Plain for ReadCapacity10 {}

impl ReadCapacity10 {
    pub const fn new(control: u8) -> Self {
        Self {
            opcode: Opcode::ReadCapacity10 as u8,
            _rsvd1: 0,
            obsolete_lba: 0,
            _rsvd2: [0; 3],
            control,
        }
    }
}
// TODO: ReadCapacity16

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ReadCapacity10ParamData {
    pub max_lba: u32,
    pub block_len: u32,
}
unsafe impl plain::Plain for ReadCapacity10ParamData {}

impl ReadCapacity10ParamData {
    pub const fn block_count(&self) -> u32 {
        u32::from_be(self.max_lba)
    }
    pub const fn logical_block_len(&self) -> u32 {
        u32::from_be(self.block_len)
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct RwErrorRecoveryPage {
    pub a: u8,
    pub page_length: u8,
    pub b: u8,
    pub read_retry_count: u8,
    _obsolete: [u8; 3],
    _rsvd: u8,
    pub recovery_time_limit: u16,
}
unsafe impl plain::Plain for RwErrorRecoveryPage {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct CachingModePage {
    pub a: u8,
    pub page_length: u8,
    // TODO: more
}
unsafe impl plain::Plain for CachingModePage {}

pub(crate) struct ModePageIterRaw<'a> {
    buffer: &'a [u8],
}

impl<'a> Iterator for ModePageIterRaw<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.buffer.len() < 2 {
            return None;
        }

        let a = self.buffer[0];
        let page_len = if a & (1 << 6) == 0 {
            // item is page_0 mode
            self.buffer[1] as usize + 1
        } else {
            // item is sub_page mode
            u16::from_be_bytes((&self.buffer[2..3]).try_into().ok()?) as usize + 3
        };
        if self.buffer.len() < page_len {
            return None;
        }
        let buffer = &self.buffer[..page_len];

        self.buffer = if page_len == self.buffer.len() {
            &[]
        } else {
            &self.buffer[page_len..]
        };

        Some(buffer)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AnyModePage<'a> {
    RwErrorRecovery(&'a RwErrorRecoveryPage),
    Caching(&'a CachingModePage),
}

struct ModePageIter<'a> {
    raw: ModePageIterRaw<'a>,
}

impl<'a> Iterator for ModePageIter<'a> {
    type Item = AnyModePage<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let next_buf = self.raw.next()?;
        let a = next_buf[0];

        let page_code = a & 0x1F;
        let spf = a & (1 << 6) != 0;

        if !spf {
            if page_code == 0x01 {
                Some(AnyModePage::RwErrorRecovery(
                    plain::from_bytes(next_buf).ok()?,
                ))
            } else if page_code == 0x08 {
                Some(AnyModePage::Caching(plain::from_bytes(next_buf).ok()?))
            } else {
                println!("Unimplemented sub_page {}", base64::encode(next_buf));
                None
            }
        } else {
            println!("Unimplemented page_0 {}", base64::encode(next_buf));
            None
        }
    }
}

pub fn mode_page_iter(buffer: &[u8]) -> impl Iterator<Item = AnyModePage<'_>> {
    ModePageIter {
        raw: ModePageIterRaw { buffer },
    }
}
