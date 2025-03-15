use pci_types::capability::PciCapabilityAddress;
use pci_types::ConfigRegionAccess;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct VendorSpecificCapability {
    pub data: Vec<u8>,
}

impl VendorSpecificCapability {
    pub unsafe fn parse(addr: PciCapabilityAddress, access: &dyn ConfigRegionAccess) -> Self {
        let dword = access.read(addr.address, addr.offset);
        let length = ((dword >> 16) & 0xFF) as u16;
        // let next = (dword >> 8) & 0xFF;
        // log::trace!(
        //     "Vendor specific offset: {:#02x} next: {next:#02x} cap len: {length:#02x}",
        //     addr.offset
        // );
        let data = if length > 0 {
            assert!(
                length > 3 && length % 4 == 0,
                "invalid range length: {}",
                length
            );
            let mut raw_data = {
                (addr.offset..addr.offset + length)
                    .step_by(4)
                    .flat_map(|offset| access.read(addr.address, offset).to_le_bytes())
                    .collect::<Vec<u8>>()
            };
            raw_data.drain(3..).collect()
        } else {
            log::warn!("Vendor specific capability is invalid");
            Vec::new()
        };
        VendorSpecificCapability { data }
    }
}
