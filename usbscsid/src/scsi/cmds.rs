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
impl ReportIdentInfo {
    pub fn new(alloc_len: u32, info_ty: ReportIdInfoInfoTy, control: u8) -> Self {
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
    IndicateSupport,

}

#[repr(packed)]
pub struct AllCommandsParam {
    /// Little endian
    pub data_len: u32,
    pub descs: [CommandDescriptor],
}

#[repr(packed)]
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
}
#[repr(packed)]
pub struct OneCommandParam {
    pub _rsvd: u8,
    /// bits 2:0 for SUPPOR, bits 6:3 reserved, and bit 7 for CTDP
    pub a: u8,
    // TODO
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

impl Inquiry {
    pub fn new(evpd: bool, page_code: u8, alloc_len: u16, control: u8) -> Self {
        Self {
            opcode: Opcode::Inquiry as u8,
            evpd: evpd as u8,
            page_code,
            alloc_len: u16::to_le(alloc_len),
            control,
        }
    }
}
