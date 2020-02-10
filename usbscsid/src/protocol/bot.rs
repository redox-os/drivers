use std::convert::TryInto;
use std::fs::File;
use std::io::prelude::*;
use std::{io, slice};

use thiserror::Error;
use xhcid_interface::{ConfDesc, DeviceReqData, EndpDirection, EndpointStatus, IfDesc, PortReqDirection, PortReqTy, PortReqRecipient, XhciClientHandle, XhciClientHandleError, XhciEndpStatusHandle};

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

pub struct BulkOnlyTransport<'a> {
    handle: &'a XhciClientHandle,
    bulk_in: File,
    bulk_out: File,
    bulk_in_status: XhciEndpStatusHandle,
    bulk_out_status: XhciEndpStatusHandle,
    bulk_in_num: u8,
    bulk_out_num: u8,
    max_lun: u8,
    current_tag: u32,
}

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
    fn recover_from_stall(&mut self) -> Result<(), ProtocolError> {
        bulk_only_mass_storage_reset(self.handle, 0)?;
        const ENDPOINT_HALT: u16 = 0;
        self.handle.clear_feature(PortReqRecipient::Endpoint, self.bulk_in_num.into(), ENDPOINT_HALT)?;
        self.handle.clear_feature(PortReqRecipient::Endpoint, self.bulk_out_num.into(), ENDPOINT_HALT)?;

        if self.bulk_in_status.current_status()? == EndpointStatus::Halted || self.bulk_out_status.current_status()? == EndpointStatus::Halted {
            return Err(ProtocolError::RecoveryFailed)
        }
        Ok(())
    }
}

impl<'a> Protocol for BulkOnlyTransport<'a> {
    fn send_command(&mut self, cb: &[u8]) -> Result<(), ProtocolError> {
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
            data_transfer_len: 256, // TODO
            lun: 0, // TODO
            flags: 1 << 7, // TODO
            cb_len: cb.len().try_into().or(Err(ProtocolError::TooLargeCommandBlock(cb.len())))?,
            command_block,
        };
        let bytes_written = self.bulk_out.write(unsafe { plain::as_bytes(&cbw) })?;
        if bytes_written != 31 {
            panic!("invalid number of cbw bytes written");
        }
        let mut buffer = [0u8; 256];
        let bytes_read = self.bulk_in.read(&mut buffer)?;
        if bytes_read != 256 {
            panic!("invalid number of bytes read");
        }
        println!("{}", base64::encode(&buffer[..]));

        let mut csw = CommandStatusWrapper::default();
        let csw_bytes_read = self.bulk_in.read(unsafe { plain::as_mut_bytes(&mut csw) })?;

        if csw_bytes_read != 13 {
            panic!("invalid number of csw bytes read");
        }
        dbg!(csw);

        if self.bulk_in_status.current_status()? == EndpointStatus::Halted || self.bulk_out_status.current_status()? == EndpointStatus::Halted {
            println!("Trying to recover from stall");
            self.recover_from_stall()?;
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
