use std::{fmt, mem, slice};
use super::opcodes::{Opcode, ServiceActionA3};

#[repr(packed)]
pub struct ReportIdentInfo {
    pub opcode: u8,
    /// bits 7:5 reserved
    pub serviceaction: u8,
    pub _rsvd: u16,
    pub restricted: u16,
    /// little endian
    pub alloc_len: u32,
    /// bit 0 reserved
    pub info_ty: u8,
    pub control: u8,
}
unsafe impl plain::Plain for ReportIdentInfo {}

impl ReportIdentInfo {
    pub const fn new(alloc_len: u32, info_ty: ReportIdInfoInfoTy, control: u8) -> Self {
        Self {
            opcode: Opcode::ServiceActionA3 as u8,
            serviceaction: ServiceActionA3::ReportIdentInfo as u8,
            _rsvd: 0,
            restricted: 0,
            alloc_len: u32::to_le(alloc_len),
            info_ty: (info_ty as u8) << REP_ID_INFO_INFO_TY_SHIFT,
            control,
        }
    }
}
#[repr(u8)]
pub enum ReportIdInfoInfoTy {
    PeripheralDevIdInfo = 0b000_0000,
    PeripheralDevTextIdInfo = 0b000_0010,
    IdentInfoSupp = 0b111_1111,
    // every other number ending with a 1 is restricted
}

pub const REP_ID_INFO_INFO_TY_MASK: u8 = 0xFE;
pub const REP_ID_INFO_INFO_TY_SHIFT: u8 = 1;

#[repr(packed)]
pub struct ReportSuppOpcodes {
    pub opcode: u8, 
    /// bits 7:5 reserved
    pub serviceaction: u8,
    /// bits 2:0 represent "REPORTING OPTIONS", bits 6:3 are reserved, and bit 7 is RCTD
    pub rep_opts: u8,
    pub req_opcode: u8,
    /// little endian
    pub req_serviceaction: u16,
    /// little endian
    pub alloc_len: u32,
    pub _rsvd: u8,
    pub control: u8,
}
unsafe impl plain::Plain for ReportSuppOpcodes {}

impl ReportSuppOpcodes {
    pub const fn new(rep_opts: ReportSuppOpcodesOptions, rctd: bool, req_opcode: u8, req_serviceaction: u16, alloc_len: u32, control: u8) -> Self {
        Self {
            opcode: Opcode::ServiceActionA3 as u8,
            serviceaction: ServiceActionA3::ReportSuppOpcodes as u8,
            rep_opts: ((rctd as u8) << REP_OPTS_RCTD_SHIFT) | rep_opts as u8,
            req_opcode,
            req_serviceaction: u16::to_le(req_serviceaction),
            alloc_len: u32::to_le(alloc_len),
            _rsvd: 0,
            control,
        }
    }
    pub const fn get_all(rctd: bool, alloc_len: u32, control: u8) -> Self {
        Self::new(ReportSuppOpcodesOptions::ListAll, rctd, 0, 0, alloc_len, control)
    }
    pub const fn get_supp_no_sa(rctd: bool, opcode: Opcode, alloc_len: u32, control: u8) -> Self {
        Self::new(ReportSuppOpcodesOptions::NoServicaction, rctd, opcode as u8, 0, alloc_len, control)
    }
    pub const fn get_supp(rctd: bool, opcode: Opcode, serviceaction: u16, alloc_len: u32, control: u8) -> Self {
        Self::new(ReportSuppOpcodesOptions::ExplicitBoth, rctd, opcode as u8, serviceaction, alloc_len, control)
    }
}

pub const REP_OPTS_MAIN_MASK: u8 = 0b0000_0111;
pub const REP_OPTS_MAIN_SHIFT: u8 = 0;
pub const REP_OPTS_RCTD_BIT: u8 = 1 << REP_OPTS_RCTD_SHIFT;
pub const REP_OPTS_RCTD_SHIFT: u8 = 7;

/// Valid values of the `req_opts` field of `ReportSuppOpcodes`.
#[repr(u8)]
pub enum ReportSuppOpcodesOptions {
    /// Returns all commands, no matter what parameters are set.
    ListAll,

    /// Returns one command with the requested opcode. If the command has service actions, this
    /// command fails.
    NoServicaction,

    /// Returns one command with the requested opcode and service action. If the command doesn't
    /// support service actions, this command fails.
    ExplicitBoth,

    /// Returns one command with the requested opcode and service action. The command may or may
    /// not implement service actions, but if it does, it has to be correct for the return value to
    /// indicate SUPPORTED.
    ///
    /// This option seems to be reserved for SPC-3 (inquiry version value of 5).
    IndicateSupport,

}

#[repr(packed)]
pub struct AllCommandsParam {
    /// Little endian
    pub data_len: u32,
    pub descs: [CommandDescriptor; 0],
}

impl AllCommandsParam {
    pub const fn alloc_len(&self) -> u32 {
        3 + u32::from_le(self.data_len)
    }
    pub unsafe fn descs(&self) -> &[CommandDescriptor] {
        assert_eq!(mem::size_of::<CommandDescriptor>(), 20);
        slice::from_raw_parts(&self.descs as *const CommandDescriptor, (self.alloc_len() as usize - 4) / mem::size_of::<CommandDescriptor>())
    }
}

unsafe impl plain::Plain for AllCommandsParam {}

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct CommandDescriptor {
    pub opcode: u8,
    pub _rsvd1: u8,
    /// little endian
    pub serviceaction: u16,
    pub _rsvd2: u8,
    /// bit 0 is SERVACTV, bit 1 is CTDP, and bits 7:2 reserved
    pub a: u8,
    /// little endian
    pub cdb_len: u16,
    pub cmd_timeouts_desc: [u8; 12],
}

#[repr(packed)]
pub struct OneCommandParam {
    pub _rsvd: u8,
    pub a: u8,
    pub cdb_size: u16,
    pub usage_data: [u8; 0],
}
unsafe impl plain::Plain for OneCommandParam {}

impl OneCommandParam {
    pub const fn ctdp(&self) -> bool {
        self.a & (1 << 7) != 0
    }
    pub fn support(&self) -> OneCommandParamSupport {
        let raw = self.a & 0b111;
        // Safe because all possible values are covered by the enum.
        unsafe { mem::transmute(raw) }
    }
    pub const fn total_len(&self) -> u16 {
        self.cdb_size as u16 + 3
    }
    /// Unsafe because the reference to self has to be valid for an additional (self.cdb_size - 1) bytes.
    pub unsafe fn cdb_usage_data(&self) -> &[u8] {
        slice::from_raw_parts(&self.usage_data as *const u8, self.total_len() as usize - 4)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OneCommandParamSupport {
    NoDataAvail = 0b000,
    NotSupported = 0b001,
    Rsvd2 = 0b010,
    Supported = 0b011,
    Rsvd4 = 0b100,
    SupportedVendor = 0b101,
    Rsvd6 = 0b110,
    Rsvd7 = 0b111,
}

#[repr(packed)]
pub struct Inquiry {
    pub opcode: u8,
    /// bits 7:2 are reserved, bit 1 (CMDDT) is obsolete, bit 0 is EVPD
    pub evpd: u8,
    pub page_code: u8,
    /// little endian
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
            alloc_len: u16::to_le(alloc_len),
            control,
        }
    }
}

#[repr(packed)]
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

#[repr(packed)]
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

#[repr(packed)]
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
    pub sense_key_specific: [u8; 3], // little endian
    pub add_sense_bytes: [u8; 0],
}
unsafe impl plain::Plain for FixedFormatSenseData {}

impl FixedFormatSenseData {
    pub const fn additional_len(&self) -> u16 {
        self.add_sense_len as u16 + 7
    }
    pub unsafe fn add_sense_bytes(&self) -> &[u8] {
        slice::from_raw_parts(&self.add_sense_len as *const u8, self.add_sense_len as usize - 18)
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

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct Read16 {
    pub opcode: u8,
    pub a: u8,
    pub lba: u64,
    pub transfer_len: u32,
    pub b: u8,
    pub control: u8,
}

impl Read16 {
    pub const fn new(lba: u64, transfer_len: u32, control: u8) -> Self {
        // TODO: RDPROTECT, DPO, FUA, RARC
        // TODO: DLD
        // TODO: Group number
        Self {
            opcode: Opcode::Read16 as u8,
            a: 0,
            lba: u64::to_le(lba),
            transfer_len: u32::to_le(transfer_len),
            b: 0,
            control,
        }
    }
}

#[repr(packed)]
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
    pub const fn new(dbd: bool, page_code: u8, pc: u8, subpage_code: u8, alloc_len: u8, control: u8) -> Self {
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

#[repr(packed)]
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
    pub const fn new(dbd: bool, llbaa: bool, page_code: u8, pc: ModePageControl, subpage_code: u8, alloc_len: u16, control: u8) -> Self {
        Self {
            opcode: Opcode::ModeSense10 as u8,
            a: ((dbd as u8) << 3) | ((llbaa as u8) << 4),
            b: page_code | ((pc as u8) << 6),
            subpage_code,
            _rsvd: [0u8; 3],
            alloc_len: u16::from_le(alloc_len),
            control,
        }
    }
    pub const fn get_block_desc(alloc_len: u16, control: u8) -> Self {
        Self::new(false, true, 0x3F, ModePageControl::CurrentValues, 0x00, alloc_len, control)
    }
}

#[repr(u8)]
pub enum ModePageControl {
    CurrentValues,
    ChangeableChanges,
    DefaultValues,
    SavedValue,
}

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct ShortLbaModeParamBlkDesc {
    pub block_count: u32,
    _rsvd: u8,
    pub logical_block_len: [u8; 3],
}
unsafe impl plain::Plain for ShortLbaModeParamBlkDesc {}

impl ShortLbaModeParamBlkDesc {
    pub const fn block_count(&self) -> u32 {
        u32::from_le(self.block_count)
    }
    pub const fn logical_block_len(&self) -> u32 {
        u24_le_to_u32(self.logical_block_len)
    }
}

const fn u24_le_to_u32(u24: [u8; 3]) -> u32 {
        ((u24[0] as u32) << 16)
            | ((u24[1] as u32) << 8)
            | (u24[2] as u32)
}

/// From SPC-3, when LONGLBA is set. For newer devices, `ShortLbaModeParamBlkDesc` is used instead (I
/// think).
#[repr(packed)]
#[derive(Clone, Copy)]
pub struct GeneralModeParamBlkDesc {
    pub density_code: u8,
    pub block_count: [u8; 3],
    _rsvd: u8,
    pub block_length: [u8; 3],
}
unsafe impl plain::Plain for GeneralModeParamBlkDesc {}

impl fmt::Debug for GeneralModeParamBlkDesc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("GeneralModeParamBlkDesc")
            .field("density_code", &self.density_code)
            .field("block_count", &u24_le_to_u32(self.block_count))
            .field("block_length", &u24_le_to_u32(self.block_length))
            .finish()
    }
}

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct LongLbaModeParamBlkDesc {
    pub block_count: u64,
    _rsvd: u32,
    pub logical_block_len: u32,
}
unsafe impl plain::Plain for LongLbaModeParamBlkDesc {}

impl LongLbaModeParamBlkDesc {
    pub const fn block_count(&self) -> u64 {
        u64::from_le(self.block_count)
    }
    pub const fn logical_block_len(&self) -> u32 {
        u32::from_le(self.logical_block_len)
    }
}

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct ModeParamHeader6 {
    pub mode_data_len: u8,
    pub medium_ty: u8,
    pub a: u8,
    pub block_desc_len: u8,
}
unsafe impl plain::Plain for ModeParamHeader6 {}

#[repr(packed)]
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
        u16::from_le(self.mode_data_len)
    }
    pub const fn block_desc_len(&self) -> u16 {
        u16::from_le(self.block_desc_len)
    }
    pub const fn longlba(&self) -> bool {
        (self.b & 0x01) != 0
    }
}
