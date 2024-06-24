use std::fs::File;

use pcid_interface::*;

use crate::{transport::Error, Device};

pub fn enable_msix(pcid_handle: &mut PciFunctionHandle) -> Result<File, Error> {
    unimplemented!("virtio_core: aarch64 enable_msix")
}
