use std::num::NonZeroU32;
use std::slice;

use xhcid_interface::{
    ConfDesc, DeviceReqData, EndpBinaryDirection, EndpDirection, EndpointStatus, IfDesc, Invalid,
    PortReqRecipient, PortReqTy, PortTransferStatus, XhciClientHandle, XhciClientHandleError,
    XhciEndpHandle,
};

use super::{Protocol, ProtocolError, SendCommandStatus, SendCommandStatusKind};

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
    pub lun: u8,   // bits 7:5 reserved
    pub cb_len: u8,
    pub command_block: [u8; 16],
}
impl CommandBlockWrapper {
    pub fn new(
        tag: u32,
        data_transfer_len: u32,
        direction: EndpBinaryDirection,
        lun: u8,
        cb: &[u8],
    ) -> Result<Self, ProtocolError> {
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
    bulk_in: XhciEndpHandle,
    bulk_out: XhciEndpHandle,
    bulk_in_num: u8,
    bulk_out_num: u8,
    max_lun: u8,
    current_tag: u32,
    interface_num: u8,
}

pub const FEATURE_ENDPOINT_HALT: u16 = 0;

impl<'a> BulkOnlyTransport<'a> {
    pub fn init(
        handle: &'a XhciClientHandle,
        config_desc: &ConfDesc,
        if_desc: &IfDesc,
    ) -> Result<Self, ProtocolError> {
        let endpoints = &if_desc.endpoints;

        let bulk_in_num = (endpoints
            .iter()
            .position(|endpoint| endpoint.direction() == EndpDirection::In)
            .unwrap()
            + 1) as u8;
        let bulk_out_num = (endpoints
            .iter()
            .position(|endpoint| endpoint.direction() == EndpDirection::Out)
            .unwrap()
            + 1) as u8;

        let max_lun = get_max_lun(handle, 0)?;
        println!("BOT_MAX_LUN {}", max_lun);

        Ok(Self {
            bulk_in: handle.open_endpoint(bulk_in_num)?,
            bulk_out: handle.open_endpoint(bulk_out_num)?,
            bulk_in_num,
            bulk_out_num,
            handle,
            max_lun,
            current_tag: 0,
            interface_num: if_desc.number,
        })
    }
    fn clear_stall_in(&mut self) -> Result<(), XhciClientHandleError> {
        self.bulk_in.reset(false)?;
        self.handle.clear_feature(
            PortReqRecipient::Endpoint,
            u16::from(self.bulk_in_num),
            FEATURE_ENDPOINT_HALT,
        )
    }
    fn clear_stall_out(&mut self) -> Result<(), XhciClientHandleError> {
        self.bulk_out.reset(false)?;
        self.handle.clear_feature(
            PortReqRecipient::Endpoint,
            u16::from(self.bulk_out_num),
            FEATURE_ENDPOINT_HALT,
        )
    }
    fn reset_recovery(&mut self) -> Result<(), ProtocolError> {
        bulk_only_mass_storage_reset(self.handle, self.interface_num.into())?;
        self.clear_stall_in()?;
        self.clear_stall_out()?;

        if self.bulk_in.status()? == EndpointStatus::Halted
            || self.bulk_out.status()? == EndpointStatus::Halted
        {
            return Err(ProtocolError::RecoveryFailed);
        }
        Ok(())
    }
}

impl<'a> Protocol for BulkOnlyTransport<'a> {
    fn send_command(
        &mut self,
        cb: &[u8],
        data: DeviceReqData,
    ) -> Result<SendCommandStatus, ProtocolError> {
        self.current_tag += 1;
        let tag = self.current_tag;

        println!("{}", base64::encode(cb));
        println!();

        let mut cbw_bytes = [0u8; 31];
        let cbw = plain::from_mut_bytes::<CommandBlockWrapper>(&mut cbw_bytes).unwrap();
        *cbw = CommandBlockWrapper::new(tag, data.len() as u32, data.direction().into(), 0, cb)?;
        let cbw = *cbw;
        println!("{}", base64::encode(&cbw_bytes));

        dbg!(self.bulk_in.status()?, self.bulk_out.status()?);

        bulk_only_mass_storage_reset(&self.handle, self.interface_num.into())?;

        match self.bulk_out.transfer_write(&cbw_bytes)? {
            PortTransferStatus::Stalled => {
                println!("bulk out endpoint stalled when sending CBW");
                self.clear_stall_out()?;
                dbg!(self.bulk_in.status()?, self.bulk_out.status()?);
            }
            PortTransferStatus::ShortPacket(n) if n != 31 => {
                panic!("received short packet when sending CBW ({} != 31)", n);
            }
            _ => (),
        }

        match data {
            DeviceReqData::In(buffer) => {
                match self.bulk_in.transfer_read(buffer)? {
                    PortTransferStatus::Success => (),
                    PortTransferStatus::ShortPacket(len) => {
                        panic!("received short packed (len {}) when transferring data", len)
                    }
                    PortTransferStatus::Stalled => {
                        println!("bulk in endpoint stalled when reading data");
                        self.clear_stall_in()?;
                    }
                    PortTransferStatus::Unknown => {
                        return Err(ProtocolError::XhciError(
                            XhciClientHandleError::InvalidResponse(Invalid(
                                "unknown transfer status",
                            )),
                        ))
                    }
                };
                println!("{}", base64::encode(&buffer[..]));
            }
            DeviceReqData::Out(buffer) => match self.bulk_out.transfer_write(buffer)? {
                PortTransferStatus::Success => (),
                PortTransferStatus::ShortPacket(len) => {
                    panic!("received short packed (len {}) when transferring data", len)
                }
                PortTransferStatus::Stalled => {
                    println!("bulk out endpoint stalled when reading data");
                    self.clear_stall_out()?;
                }
                PortTransferStatus::Unknown => {
                    return Err(ProtocolError::XhciError(
                        XhciClientHandleError::InvalidResponse(Invalid("unknown transfer status")),
                    ))
                }
            },
            DeviceReqData::NoData => (),
        }

        let mut csw_buffer = [0u8; 13];

        match self.bulk_in.transfer_read(&mut csw_buffer)? {
            PortTransferStatus::Stalled => {
                println!("bulk in endpoint stalled when reading CSW");
                self.clear_stall_in()?;
            }
            PortTransferStatus::ShortPacket(n) if n != 13 => {
                panic!("received a short packet when reading CSW ({} != 13)", n)
            }
            _ => (),
        };
        println!("{}", base64::encode(&csw_buffer));
        let csw = plain::from_bytes::<CommandStatusWrapper>(&csw_buffer).unwrap();

        dbg!(csw);
        if !csw.is_valid() || csw.tag != cbw.tag {
            self.reset_recovery()?;
        }

        if self.bulk_in.status()? == EndpointStatus::Halted
            || self.bulk_out.status()? == EndpointStatus::Halted
        {
            println!("Trying to recover from stall");
            dbg!(self.bulk_in.status()?, self.bulk_out.status()?);
        }

        Ok(SendCommandStatus {
            kind: if csw.status == CswStatus::Passed as u8 {
                SendCommandStatusKind::Success
            } else if csw.status == CswStatus::Failed as u8 {
                SendCommandStatusKind::Failed
            } else {
                return Err(ProtocolError::ProtocolError(
                    "bulk-only transport phase error, or other",
                ));
            },
            residue: NonZeroU32::new(csw.data_residue),
        })
    }
}

pub fn bulk_only_mass_storage_reset(
    handle: &XhciClientHandle,
    if_num: u16,
) -> Result<(), XhciClientHandleError> {
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        0xFF,
        0,
        if_num,
        DeviceReqData::NoData,
    )
}
pub fn get_max_lun(handle: &XhciClientHandle, if_num: u16) -> Result<u8, XhciClientHandleError> {
    let mut lun = 0u8;
    let buffer = slice::from_mut(&mut lun);
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        0xFE,
        0,
        if_num,
        DeviceReqData::In(buffer),
    )?;
    Ok(lun)
}
