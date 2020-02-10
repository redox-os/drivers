use std::io;

use thiserror::Error;
use xhcid_interface::{DeviceReqData, DevDesc, ConfDesc, IfDesc, XhciClientHandle, XhciClientHandleError};

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
}

pub trait Protocol {
    fn send_command(&mut self, command: &[u8], data: DeviceReqData) -> Result<(), ProtocolError>;
}

/// Bulk-only transport
pub mod bot;

mod uas {
    // TODO
}

use bot::BulkOnlyTransport;

pub fn setup<'a>(handle: &'a XhciClientHandle, protocol: u8, dev_desc: &DevDesc, conf_desc: &ConfDesc, if_desc: &IfDesc) -> Option<Box<dyn Protocol + 'a>> {
    match protocol {
        0x50 => Some(Box::new(BulkOnlyTransport::init(handle, conf_desc, if_desc).unwrap())),
        _ => None,
    }
}
