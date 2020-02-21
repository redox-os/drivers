use plain::Plain;

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EndpointDescriptor {
    pub length: u8,
    pub kind: u8,
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

pub const ENDP_ATTR_TY_MASK: u8 = 0x3;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndpointTy {
    Ctrl = 0,
    Interrupt = 1,
    Bulk = 2,
    Isoch = 3,
}

impl EndpointDescriptor {
    fn ty(self) -> EndpointTy {
        match self.attributes & ENDP_ATTR_TY_MASK {
            0 => EndpointTy::Ctrl,
            1 => EndpointTy::Interrupt,
            2 => EndpointTy::Bulk,
            3 => EndpointTy::Isoch,
            _ => unreachable!(),
        }
    }
}

unsafe impl Plain for EndpointDescriptor {}

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SuperSpeedCompanionDescriptor {
    pub length: u8,
    pub kind: u8,
    pub max_burst: u8,
    pub attributes: u8,
    pub bytes_per_interval: u16,
}
unsafe impl Plain for SuperSpeedCompanionDescriptor {}

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SuperSpeedPlusIsochCmpDescriptor {
    pub length: u8,
    pub kind: u8,
    pub reserved: u16,
    pub bytes_per_interval: u32,
}
unsafe impl Plain for SuperSpeedPlusIsochCmpDescriptor {}

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HidDescriptor {
    pub length: u8,
    pub kind: u8,
    pub hid_spec_release: u16,
    pub country_code: u8,
    pub num_descriptors: u8,
    pub report_desc_ty: u8,
    pub report_desc_len: u16,
    pub optional_desc_ty: u8,
    pub optional_desc_len: u16,
}

unsafe impl Plain for HidDescriptor {}
