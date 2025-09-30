pub mod device;

use driver_udc::{UDCAdapter, UDCScheme};
use syscall::{
    Error, EventFlags, Result, Stat, EACCES, EAGAIN, EBADF, EINTR, EINVAL, EWOULDBLOCK, MODE_FILE,
};
use event::{user_data, EventQueue};

pub struct DWC3 {

}

impl DWC3 {
    pub fn new() -> Self {
        DWC3 {}
    }
}

impl UDCAdapter for DWC3 {
    fn write_ep(&mut self, ep: usize, buf: &[u8]) -> Result<usize> {
        todo!("Todo");
    }

    fn read_ep(&mut self, ep: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        todo!("Todo");
    }
}

pub fn dwc3_init(dev_name: String, address: usize) -> Result<UDCScheme<DWC3>> {
    let scheme_name = format!("udc.{}", dev_name);
    let dwc3 = DWC3::new();
    let scheme = UDCScheme::new(
        dwc3,
        scheme_name,
        address,
    );

    Ok(scheme)
}
