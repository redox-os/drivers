use std::io;
use std::num::NonZeroU32;

use thiserror::Error;
use xhcid_interface::{
    ConfDesc, DevDesc, DeviceReqData, IfDesc, XhciClientHandle, XhciClientHandleError,
};

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Too large command block ({0} > 16)")]
    TooLargeCommandBlock(usize),

    #[error("xhcid connection error: {0}")]
    XhciError(#[from] XhciClientHandleError),

    #[error("i/o error")]
    IoError(#[from] io::Error),

    #[error("attempted recovery failed")]
    RecoveryFailed,

    #[error("protocol error")]
    ProtocolError(&'static str),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SendCommandStatus {
    pub residue: Option<NonZeroU32>,
    pub kind: SendCommandStatusKind,
}

impl SendCommandStatus {
    pub fn bytes_transferred(&self, transfer_len: u32) -> u32 {
        transfer_len - self.residue.map(u32::from).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SendCommandStatusKind {
    Success,
    Failed,
}

impl Default for SendCommandStatusKind {
    fn default() -> Self {
        Self::Success
    }
}

pub trait Protocol {
    fn send_command(
        &mut self,
        command: &[u8],
        data: DeviceReqData,
    ) -> Result<SendCommandStatus, ProtocolError>;
}

/// Bulk-only transport
pub mod bot;

mod uas {
    // TODO
}

use bot::BulkOnlyTransport;

pub fn setup<'a>(
    handle: &'a XhciClientHandle,
    protocol: u8,
    dev_desc: &DevDesc,
    conf_desc: &ConfDesc,
    if_desc: &IfDesc,
) -> Option<Box<dyn Protocol + 'a>> {
    match protocol {
        0x50 => Some(Box::new(
            BulkOnlyTransport::init(handle, conf_desc, if_desc).unwrap(),
        )),
        _ => None,
    }
}
