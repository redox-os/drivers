//! Implements the "Device" USB Descriptor.
//!
//! This descriptor is described in USB32 section 9.6.1

/// A USB Device Descriptor.
///
/// This is common to all USB standards, and "provides information that applies globally to the
/// device and all the device's configurations" (USB32 9.6.1)
///
/// A given device will only have one device descriptor.
///
/// USB32 Table 9-11 describes the USB packet offsets of the fields described by this structure.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceDescriptor {
    /// The length of this descriptor in bytes.
    /// The bLength field in USB32 Table 9-11
    pub length: u8,
    /// The descriptor type. See [DescriptorKind]
    /// The bDescriptorType field in USB32 Table 9-11.
    pub kind: u8,
    /// The USB standard version in binary-coded decimal.
    ///
    /// USB 2.1 would be encoded as 210H, 3.2 would be 320H.
    /// The bcdUSB field in USB32 Table 9-11
    pub usb: u16,
    /// The USB Class Code.
    ///
    /// bDeviceClass in USB32 Table 9-11.
    ///
    /// These are values assigned by USB-IF that describes the type of device connected via USB.
    ///
    /// A value of FF indicates a vendor-specific class. A value of 0 indicates that all the
    /// interfaces in a configuration will provide their own class information.
    pub class: u8,
    /// The USB Sub Device Class Code.
    ///
    /// bDeviceSubClass in USB32 Table 9-11
    ///
    /// These specify subclasses of a device class specified by the 'class' field.
    pub sub_class: u8,
    /// The USB Protocol code.
    ///
    /// bDeviceProtocol in USB32 Table 9-11
    ///
    /// This qualified by the class and sub_class fields, and specifies the application-layer protocol
    /// (the protocol encapsulated by USB) of this device.
    pub protocol: u8,
    /// The maximum packet size for endpoint 0.
    ///
    /// bMaxPacketSize0 in USB32 Table 9-11
    pub packet_size: u8,
    /// The USB Vendor ID
    ///
    /// idVendor in USB32 Table 9-11
    pub vendor: u16,
    /// The USB Product ID
    ///
    /// idProduct in USB32 Table 9-11
    pub product: u16,
    /// The device release number in binary-coded decimal.
    ///
    /// bcdDevice in USB32 Table 9-11
    pub release: u16,
    /// Index of the String Descriptor describing the device manufacturer
    ///
    /// iManufacturer in USB32 Table 9-11
    pub manufacturer_str: u8,
    /// Index of the String Descriptor describing the product
    ///
    /// iProduct in Table 9-11
    pub product_str: u8,
    /// Index of the string descriptor describing the device's serial number
    ///
    /// iSerialNumber in USB32 Table 9-11
    pub serial_str: u8,
    /// The number of possible configurations (Configuration Descriptors) for this device.
    ///
    /// bNumConfigurations in USB32 Table 9-11
    pub configurations: u8,
}

unsafe impl plain::Plain for DeviceDescriptor {}

impl DeviceDescriptor {
    /// Gets the USB Minor Version
    pub fn minor_usb_vers(&self) -> u8 {
        (self.usb & 0xFF) as u8
    }
    /// Gets the USB Major Version
    pub fn major_usb_vers(&self) -> u8 {
        ((self.usb >> 8) & 0xFF) as u8
    }
}

/// The 8-byte version of the Device Descriptor
///
/// This is a subset of the full Device Descriptor. When the system is first performing device
/// enumeration, it will request only the first eight bytes of the DeviceDescriptor from each
/// device as this contains the crucial information, and then it will request the full descriptor
/// at a later point.
///
/// See [DeviceDescriptor]
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceDescriptor8Byte {
    /// See [DeviceDescriptor]
    pub length: u8,
    /// See [DeviceDescriptor]
    pub kind: u8,
    /// See [DeviceDescriptor]
    pub usb: u16,
    /// See [DeviceDescriptor]
    pub class: u8,
    /// See [DeviceDescriptor]
    pub sub_class: u8,
    /// See [DeviceDescriptor]
    pub protocol: u8,
    /// See [DeviceDescriptor]
    pub packet_size: u8,
}

unsafe impl plain::Plain for DeviceDescriptor8Byte {}

impl DeviceDescriptor8Byte {
    /// Gets the USB Minor Version
    pub fn minor_usb_vers(&self) -> u8 {
        (self.usb & 0xFF) as u8
    }

    /// Gets the USB Major Version
    pub fn major_usb_vers(&self) -> u8 {
        ((self.usb >> 8) & 0xFF) as u8
    }
}

/// A Device Qualifier Descriptor
///
/// This is a descriptor specific to the USB2 standard, and was deprecated in USB3. USB2 devices
/// will still provide this value.
///
/// A Device Qualifier is sent by a high-speed capable USB2 device to describe information in its
/// descriptor that would change if it was operating at the other speed. If it was at low speed,
/// the qualifier would describe the device at high speed. If it was at high speed, the qualifier
/// would describe the device at low speed.
///
/// See USB2 section 9.6.2
///
/// The packet offsets are described in USB2 Table 9-9
#[repr(C, packed)]
pub struct DeviceQualifier {
    /// The size of the descriptor.
    ///
    /// bLength in USB2 Table 9-9
    pub length: u8,
    /// The Device Descriptor Type (see [xhci_interface::usb::DescriptorKind])
    ///
    /// bDescriptorType in USB2 Table 9-9
    pub kind: u8,
    /// The USB specification version number in binary-coded decimal
    ///
    /// bDeviceClass in USB2 Table 9-9
    pub usb: u16,
    /// The USB Device Class Code
    ///
    /// bDeviceClass in USB2 Table 9-9
    pub class: u8,
    /// The USB Device Sub Class Code
    ///
    /// bDeviceSubClass in USB2 Table 9-9
    pub sub_class: u8,
    /// The USB Device Protocol Code
    ///
    /// bDeviceProtocol in USB2 Table 9-9
    pub protocol: u8,
    /// The maximum packet size for the other speed\
    ///
    /// bMaxPacketSize0 in USB2 Table9-9
    pub pkgsz_other_speed: u8,
    /// The number of device configurations for the other speed
    ///
    /// bNumConfiguration in USB2 Table 9-9
    pub num_other_speed_cfgs: u8,
    /// Reserved for future use by the USB2 standard
    ///
    /// (DeviceQualifier was dropped in USB3, so it was never used!)
    /// bReserved in USB2 Table 9-9
    pub _rsvd: u8,
}

unsafe impl plain::Plain for DeviceQualifier {}
