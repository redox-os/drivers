use super::Protocol;

pub const CBW_SIGNATURE: u32 = 0x43425355;

/// 0 means host to dev, 1 means dev to host
pub const CBW_FLAGS_DIRECTION_BIT: u8 = 1 << CBW_FLAGS_DIRECTION_SHIFT;
pub const CBW_FLAGS_DIRECTION_SHIFT: u8 = 7;

#[repr(packed)]
pub struct CommandBlockWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_transfer_len: u32,
    pub flags: u8, // upper nibble reserved
    pub lun: u8, // bits 7:5 reserved
    pub cb_len: u8,
    pub command_block: [u8; 16],
}

pub const CSW_SIGNATURE: u32 = 0x53425355;

#[repr(u8)]
pub enum CswStatus {
    Passed = 0,
    Failed = 1,
    PhaseError = 2,
    // the rest are reserved
}

#[repr(packed)]
pub struct CommandStatusWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,
}

pub struct BulkOnlyTransport;

impl Protocol for BulkOnlyTransport {
    fn send_command_block(&mut self, cb: &[u8]) {
        todo!()
    }
    fn recv_command_block(&mut self, cb: &mut [u8]) {
        todo!()
    }
}
