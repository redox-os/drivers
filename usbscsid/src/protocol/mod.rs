use thiserror::Error;
use xhcid_interface::{XhciClientHandle, XhciClientHandleError};

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Too large command block ({0} > 16)")]
    TooLargeCommandBlock(usize),

    #[error("xhcid connection error: {0}")]
    XhciError(#[from] XhciClientHandleError),
}

pub trait Protocol {
    fn send_command_block(&mut self, cb: &[u8]) -> Result<(), ProtocolError>;
    fn recv_command_block(&mut self, cb: &mut [u8]) -> Result<(), ProtocolError>;
}

/// Bulk-only transport
pub mod bot;

mod uas {
    // TODO
}

use bot::BulkOnlyTransport;

pub fn setup<'a>(handle: &'a XhciClientHandle, protocol: u8) -> Option<Box<dyn Protocol + 'a>> {
    match protocol {
        0x50 => Some(Box::new(BulkOnlyTransport::init(handle).unwrap())),
        _ => None,
    }
}
