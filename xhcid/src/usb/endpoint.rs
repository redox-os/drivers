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

unsafe impl Plain for EndpointDescriptor {}
