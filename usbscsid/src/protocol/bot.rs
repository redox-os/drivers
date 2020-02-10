use std::convert::TryInto;
use std::fs::File;
use std::io::prelude::*;
use std::{io, slice};

use thiserror::Error;
use xhcid_interface::{ConfDesc, DeviceReqData, EndpBinaryDirection, EndpDirection, EndpointStatus, IfDesc, Invalid, PortReqDirection, PortReqTy, PortReqRecipient, PortTransferStatus, XhciClientHandle, XhciClientHandleError, XhciEndpStatusHandle, XhciEndpTransferHandle};

use super::{Protocol, ProtocolError};

pub const CBW_SIGNATURE: u32 = 0x43425355;

/// 0 means host to dev, 1 means dev to host
pub const CBW_FLAGS_DIRECTION_BIT: u8 = 1 << CBW_FLAGS_DIRECTION_SHIFT;
pub const CBW_FLAGS_DIRECTION_SHIFT: u8 = 7;

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandBlockWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_transfer_len: u32,
    pub flags: u8, // upper nibble reserved
    pub lun: u8, // bits 7:5 reserved
    pub cb_len: u8,
    pub command_block: [u8; 16],
}
impl CommandBlockWrapper {
    pub fn new(tag: u32, data_transfer_len: u32, direction: EndpBinaryDirection, lun: u8, cb: &[u8]) -> Result<Self, ProtocolError> {
        let mut command_block = [0u8; 16];
        if cb.len() > 16 {
            return Err(ProtocolError::TooLargeCommandBlock(cb.len()));
        }

        command_block[..cb.len()].copy_from_slice(&cb);
        Ok(Self {
            signature: CBW_SIGNATURE,
            tag,
            data_transfer_len,
            flags: match direction {
                EndpBinaryDirection::Out => 0,
                EndpBinaryDirection::In => 1,
            } << CBW_FLAGS_DIRECTION_SHIFT,
            lun,
            cb_len: cb.len() as u8,
            command_block,
        })
    }
}
unsafe impl plain::Plain for CommandBlockWrapper {}

pub const CSW_SIGNATURE: u32 = 0x53425355;

#[repr(u8)]
pub enum CswStatus {
    Passed = 0,
    Failed = 1,
    PhaseError = 2,
    // the rest are reserved
}

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandStatusWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,
}
unsafe impl plain::Plain for CommandStatusWrapper {}

impl CommandStatusWrapper {
    pub fn is_valid(&self) -> bool {
        self.signature == CSW_SIGNATURE
    }
}

pub struct BulkOnlyTransport<'a> {
    handle: &'a XhciClientHandle,
    bulk_in: XhciEndpTransferHandle,
    bulk_out: XhciEndpTransferHandle,
    bulk_in_status: XhciEndpStatusHandle,
    bulk_out_status: XhciEndpStatusHandle,
    bulk_in_num: u8,
    bulk_out_num: u8,
    max_lun: u8,
    current_tag: u32,
}

pub const FEATURE_ENDPOINT_HALT: u16 = 0;

impl<'a> BulkOnlyTransport<'a> {
    pub fn init(handle: &'a XhciClientHandle, config_desc: &ConfDesc, if_desc: &IfDesc) -> Result<Self, ProtocolError> {
        let endpoints = &if_desc.endpoints;

        let bulk_in_num = (endpoints.iter().position(|endpoint| endpoint.direction() == EndpDirection::In).unwrap() + 1) as u8;
        let bulk_out_num = (endpoints.iter().position(|endpoint| endpoint.direction() == EndpDirection::Out).unwrap() + 1) as u8;

        let max_lun = get_max_lun(handle, 0)?;
        println!("BOT_MAX_LUN {}", max_lun);

        Ok(Self {
            bulk_in: handle.open_endpoint(bulk_in_num, PortReqDirection::DeviceToHost)?,
            bulk_out: handle.open_endpoint(bulk_out_num, PortReqDirection::HostToDevice)?,
            bulk_in_status: handle.open_endpoint_status(bulk_in_num)?,
            bulk_out_status: handle.open_endpoint_status(bulk_out_num)?,
            bulk_in_num,
            bulk_out_num,
            handle,
            max_lun,
            current_tag: 0,
        })
    }
    fn clear_stall(&mut self, endp_num: u8) -> Result<(), XhciClientHandleError> {
        self.handle.clear_feature(PortReqRecipient::Endpoint, u16::from(endp_num), FEATURE_ENDPOINT_HALT)
    }
    fn reset_recovery(&mut self) -> Result<(), ProtocolError> {
        bulk_only_mass_storage_reset(self.handle, 0)?;
        self.clear_stall(self.bulk_in_num.into())?;
        self.clear_stall(self.bulk_out_num.into())?;

        if self.bulk_in_status.current_status()? == EndpointStatus::Halted || self.bulk_out_status.current_status()? == EndpointStatus::Halted {
            return Err(ProtocolError::RecoveryFailed)
        }
        Ok(())
    }
}

impl<'a> Protocol for BulkOnlyTransport<'a> {
    fn send_command(&mut self, cb: &[u8], data: DeviceReqData) -> Result<(), ProtocolError> {
        dbg!(self.bulk_in_status.current_status()?);
        dbg!(self.bulk_out_status.current_status()?);
        self.current_tag += 1;
        let tag = self.current_tag;

        let mut command_block = [0u8; 16];
        if cb.len() > 16 {
            return Err(ProtocolError::TooLargeCommandBlock(cb.len()));
        }
        command_block[..cb.len()].copy_from_slice(&cb);

        let cbw = CommandBlockWrapper {
            signature: CBW_SIGNATURE,
            tag,
            data_transfer_len: data.len() as u32,
            lun: 0, // TODO
            flags: u8::from(data.direction() == PortReqDirection::DeviceToHost) << 7,
            cb_len: cb.len().try_into().or(Err(ProtocolError::TooLargeCommandBlock(cb.len())))?,
            command_block,
        };
        match self.bulk_out.transfer_write(unsafe { plain::as_bytes(&cbw) })? {
            PortTransferStatus::ShortPacket(31) => (),
            PortTransferStatus::Stalled => {
                panic!("bulk out endpoint stalled when sending CBW");
            }
            _ => panic!("invalid number of CBW bytes written; expected a short packed of length 31 (0x1F)"),
        }

        match data {
            DeviceReqData::In(buffer) => {
                match self.bulk_in.transfer_read(buffer)? {
                    PortTransferStatus::Success => (),
                    PortTransferStatus::ShortPacket(len) => panic!("received short packed (len {}) when transferring data", len),
                    PortTransferStatus::Stalled => {
                        println!("bulk in endpoint stalled when reading data");
                        self.clear_stall(self.bulk_in_num)?;
                    }
                    PortTransferStatus::Unknown => return Err(ProtocolError::XhciError(XhciClientHandleError::InvalidResponse(Invalid("unknown transfer status")))),
                };
                println!("{}", base64::encode(&buffer[..]));
            }
            DeviceReqData::Out(ref buffer) => todo!(),
            DeviceReqData::NoData => todo!(),
        };

        let mut csw = CommandStatusWrapper::default();

        match self.bulk_in.transfer_read(unsafe { plain::as_mut_bytes(&mut csw) })? {
            PortTransferStatus::ShortPacket(13) => (),
            PortTransferStatus::Stalled => {
                println!("bulk in endpoint stalled when reading CSW");
                self.clear_stall(self.bulk_in_num)?;
            }
            _ => panic!("invalid number of CSW bytes read; expected a short packet of length 13 (0xD)"),
        };

        if !csw.is_valid() {
            self.reset_recovery()?;
        }
        dbg!(csw);

        if self.bulk_in_status.current_status()? == EndpointStatus::Halted || self.bulk_out_status.current_status()? == EndpointStatus::Halted {
            println!("Trying to recover from stall");
            dbg!(self.bulk_in_status.current_status()?, self.bulk_out_status.current_status()?);
        }

        Ok(())
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
