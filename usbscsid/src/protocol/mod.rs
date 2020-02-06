pub trait Protocol {
    fn send_command_block(&mut self, cb: &[u8]);
    fn recv_command_block(&mut self, cb: &mut [u8]);
}

/// Bulk-only transport
pub mod bot;

/// Control-Bulk-Interface transpoint
mod cbi {
    // TODO
}
