use std::slice;

use thiserror::Error;
use xhcid_interface::{DeviceReqData, PortReqTy, PortReqRecipient, XhciClientHandle, XhciClientHandleError};

use super::{Protocol, ProtocolError};

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

pub struct BulkOnlyTransport<'a> {
    handle: &'a XhciClientHandle,
}

impl<'a> BulkOnlyTransport<'a> {
    pub fn init(handle: &'a XhciClientHandle) -> Result<Self, ProtocolError> {
        let lun = get_max_lun(handle, 0)?;
        println!("BOT_MAX_LUN {}", lun);
        Ok(Self {
            handle,
        })
    }
}

impl<'a> Protocol for BulkOnlyTransport<'a> {
    fn send_command_block(&mut self, cb: &[u8]) -> Result<(), ProtocolError> {
        todo!()
    }
    fn recv_command_block(&mut self, cb: &mut [u8]) -> Result<(), ProtocolError> {
        todo!()
    }
}

pub fn bulk_only_mass_storage_reset(handle: &XhciClientHandle, if_num: u16) -> Result<(), XhciClientHandleError> {
    handle.device_request(PortReqTy::Class, PortReqRecipient::Interface, 0xFF, 0, if_num, DeviceReqData::NoData)
}
pub fn get_max_lun(handle: &XhciClientHandle, if_num: u16) -> Result<u8, XhciClientHandleError> {
    let mut lun = 0;
    let buffer = slice::from_mut(&mut lun);
    handle.device_request(PortReqTy::Class, PortReqRecipient::Interface, 0xFE, 0, if_num, DeviceReqData::In(buffer))?;
    Ok(lun)
}
