use std::fmt;
use std::mem::transmute;

pub type HDANodeAddr = u16;
pub type HDACodecAddr = u8;

pub type NodeAddr = u16;
pub type CodecAddr = u8;

pub type WidgetAddr = (CodecAddr, NodeAddr);
/*
impl fmt::Display for WidgetAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:01X}:{:02X}\n", self.0, self.1)
    }
}*/

#[derive(Debug, PartialEq)]
#[repr(u8)]
pub enum HDAWidgetType {
    AudioOutput = 0x0,
    AudioInput = 0x1,
    AudioMixer = 0x2,
    AudioSelector = 0x3,
    PinComplex = 0x4,
    Power = 0x5,
    VolumeKnob = 0x6,
    BeepGenerator = 0x7,

    VendorDefined = 0xf,
}

impl fmt::Display for HDAWidgetType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, PartialEq)]
#[repr(u8)]
pub enum DefaultDevice {
    LineOut = 0x0,
    Speaker = 0x1,
    HPOut = 0x2,
    CD = 0x3,
    SPDIF = 0x4,
    DigitalOtherOut = 0x5,
    ModemLineSide = 0x6,
    ModemHandsetSide = 0x7,
    LineIn = 0x8,
    AUX = 0x9,
    MicIn = 0xA,
    Telephony = 0xB,
    SPDIFIn = 0xC,
    DigitalOtherIn = 0xD,
    Reserved = 0xE,
    Other = 0xF,
}

#[derive(Debug)]
#[repr(u8)]
pub enum PortConnectivity {
    ConnectedToJack = 0x0,
    NoPhysicalConnection = 0x1,
    FixedFunction = 0x2,
    JackAndInternal = 0x3,
}

#[derive(Debug)]
#[repr(u8)]
pub enum GrossLocation {
    ExternalOnPrimary = 0x0,
    Internal = 0x1,
    SeperateChasis = 0x2,
    Other = 0x3,
}

#[derive(Debug)]
#[repr(u8)]
pub enum GeometricLocation {
    NA = 0x0,
    Rear = 0x1,
    Front = 0x2,
    Left = 0x3,
    Right = 0x4,
    Top = 0x5,
    Bottom = 0x6,
    Special1 = 0x7,
    Special2 = 0x8,
    Special3 = 0x9,
    Resvd1 = 0xA,
    Resvd2 = 0xB,
    Resvd3 = 0xC,
    Resvd4 = 0xD,
    Resvd5 = 0xE,
    Resvd6 = 0xF,
}

#[derive(Debug)]
#[repr(u8)]
pub enum Color {
    Unknown = 0x0,
    Black = 0x1,
    Grey = 0x2,
    Blue = 0x3,
    Green = 0x4,
    Red = 0x5,
    Orange = 0x6,
    Yellow = 0x7,
    Purple = 0x8,
    Pink = 0x9,
    Resvd1 = 0xA,
    Resvd2 = 0xB,
    Resvd3 = 0xC,
    Resvd4 = 0xD,
    White = 0xE,
    Other = 0xF,
}

pub struct ConfigurationDefault {
    value: u32,
}

impl ConfigurationDefault {
    pub fn from_u32(value: u32) -> ConfigurationDefault {
        ConfigurationDefault { value: value }
    }

    pub fn color(&self) -> Color {
        unsafe { transmute(((self.value >> 12) & 0xF) as u8) }
    }

    pub fn default_device(&self) -> DefaultDevice {
        unsafe { transmute(((self.value >> 20) & 0xF) as u8) }
    }

    pub fn port_connectivity(&self) -> PortConnectivity {
        unsafe { transmute(((self.value >> 30) & 0x3) as u8) }
    }

    pub fn gross_location(&self) -> GrossLocation {
        unsafe { transmute(((self.value >> 28) & 0x3) as u8) }
    }

    pub fn geometric_location(&self) -> GeometricLocation {
        unsafe { transmute(((self.value >> 24) & 0x7) as u8) }
    }

    pub fn is_output(&self) -> bool {
        match self.default_device() {
            DefaultDevice::LineOut
            | DefaultDevice::Speaker
            | DefaultDevice::HPOut
            | DefaultDevice::CD
            | DefaultDevice::SPDIF
            | DefaultDevice::DigitalOtherOut
            | DefaultDevice::ModemLineSide => true,
            _ => false,
        }
    }

    pub fn is_input(&self) -> bool {
        match self.default_device() {
            DefaultDevice::ModemHandsetSide
            | DefaultDevice::LineIn
            | DefaultDevice::AUX
            | DefaultDevice::MicIn
            | DefaultDevice::Telephony
            | DefaultDevice::SPDIFIn
            | DefaultDevice::DigitalOtherIn => true,
            _ => false,
        }
    }

    pub fn sequence(&self) -> u8 {
        (self.value & 0xF) as u8
    }

    pub fn default_association(&self) -> u8 {
        ((self.value >> 4) & 0xF) as u8
    }
}

impl fmt::Display for ConfigurationDefault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{:?} {:?} {:?} {:?}",
            self.default_device(),
            self.color(),
            self.gross_location(),
            self.geometric_location()
        )
    }
}
