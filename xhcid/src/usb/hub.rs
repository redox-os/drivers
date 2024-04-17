#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct HubDescriptor {
    pub length: u8,
    pub kind: u8,
    pub ports: u8,
    pub characteristics: u16,
    pub power_on_good: u8,
    pub current: u8,
    // device_removable: bitmap of ports, maximum of 256 bits (32 bytes)
    // power_control_mask: bitmap of ports, maximum of 256 bits (32 bytes)
    bitmaps: [u8; 64]
}

unsafe impl plain::Plain for HubDescriptor {}

impl Default for HubDescriptor {
    fn default() -> Self {
        Self {
            length: 0,
            kind: 0,
            ports: 0,
            characteristics: 0,
            power_on_good: 0,
            current: 0,
            bitmaps: [0; 64]
        }
    }
}
