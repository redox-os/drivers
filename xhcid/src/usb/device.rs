#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceDescriptor {
    pub length: u8,
    pub kind: u8,
    pub usb: u16,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub packet_size: u8,
    pub vendor: u16,
    pub product: u16,
    pub release: u16,
    pub manufacturer_str: u8,
    pub product_str: u8,
    pub serial_str: u8,
    pub configurations: u8,
}

unsafe impl plain::Plain for DeviceDescriptor {}

impl DeviceDescriptor {
    fn minor_usb_vers(&self) -> u8 {
        (self.usb & 0xFF) as u8
    }
    fn major_usb_vers(&self) -> u8 {
        ((self.usb >> 8) & 0xFF) as u8
    }
}

#[repr(packed)]
pub struct DeviceQualifier {
    pub length: u8,
    pub kind: u8,
    pub usb: u16,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub pkgsz_other_speed: u8,
    pub num_other_speed_cfgs: u8,
    pub _rsvd: u8,
}

unsafe impl plain::Plain for DeviceQualifier {}
