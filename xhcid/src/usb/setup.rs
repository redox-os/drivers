use super::DescriptorKind;

#[repr(packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Setup {
    pub kind: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    pub fn get_status() -> Self {
        Self {
            kind: 0b1000_0000,
            request: 0x00,
            value: 0,
            index: 0,
            length: 2,
        }
    }

    pub fn clear_feature(feature: u16) -> Self {
        Self {
            kind: 0b0000_0000,
            request: 0x01,
            value: feature,
            index: 0,
            length: 0,
        }
    }

    pub fn set_feature(feature: u16) -> Self {
        Self {
            kind: 0b0000_0000,
            request: 0x03,
            value: feature,
            index: 0,
            length: 0,
        }
    }

    pub fn set_address(address: u16) -> Self {
        Self {
            kind: 0b0000_0000,
            request: 0x05,
            value: address,
            index: 0,
            length: 0,
        }
    }

    pub fn get_descriptor(kind: DescriptorKind, index: u8, language: u16, length: u16) -> Self {
        Self {
            kind: 0b1000_0000,
            request: 0x06,
            value: ((kind as u16) << 8) | (index as u16),
            index: language,
            length: length,
        }
    }

    pub fn set_descriptor(kind: u8, index: u8, language: u16, length: u16) -> Self {
        Self {
            kind: 0b0000_0000,
            request: 0x07,
            value: ((kind as u16) << 8) | (index as u16),
            index: language,
            length: length,
        }
    }

    pub fn get_configuration() -> Self {
        Self {
            kind: 0b1000_0000,
            request: 0x08,
            value: 0,
            index: 0,
            length: 1,
        }
    }

    pub fn set_configuration(value: u16) -> Self {
        Self {
            kind: 0b0000_0000,
            request: 0x09,
            value: value,
            index: 0,
            length: 0,
        }
    }
}
