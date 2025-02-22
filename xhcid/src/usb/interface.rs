use plain::Plain;

///
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InterfaceDescriptor {
    pub length: u8,
    pub kind: u8,
    pub number: u8,
    pub alternate_setting: u8,
    pub endpoints: u8,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub interface_str: u8,
}

unsafe impl Plain for InterfaceDescriptor {}
