#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigDescriptor {
    pub length: u8,
    pub kind: u8,
    pub total_length: u16,
    pub interfaces: u8,
    pub configuration_value: u8,
    pub configuration_str: u8,
    pub attributes: u8,
    pub max_power: u8,
}

unsafe impl plain::Plain for ConfigDescriptor {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OtherSpeedConfig {
    pub length: u8,
    pub kind: u8,
    pub total_length: u16,
    pub interfaces: u8,
    pub configuration_value: u8,
    pub configuration_str: u8,
    pub attributes: u8,
    pub max_power: u8,
}
