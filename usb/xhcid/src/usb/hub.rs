#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct HubDescriptorV2 {
    pub length: u8,
    pub kind: u8,
    pub ports: u8,
    pub characteristics: u16,
    pub power_on_good: u8,
    pub current: u8,
    /*TODO: USB 2 and 3 disagree on the descriptor, so some fields are disabled
    // device_removable: bitmap of ports, maximum of 256 bits (32 bytes)
    // power_control_mask: bitmap of ports, maximum of 256 bits (32 bytes)
    bitmaps: [u8; 64],
    */
}

unsafe impl plain::Plain for HubDescriptorV2 {}

impl HubDescriptorV2 {
    pub const DESCRIPTOR_KIND: u8 = 0x29;
}

impl Default for HubDescriptorV2 {
    fn default() -> Self {
        Self {
            length: 0,
            kind: 0,
            ports: 0,
            characteristics: 0,
            power_on_good: 0,
            current: 0,
            /*
            bitmaps: [0; 64],
            */
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct HubDescriptorV3 {
    pub length: u8,
    pub kind: u8,
    pub ports: u8,
    pub characteristics: u16,
    pub power_on_good: u8,
    pub current: u8,
    pub decode_latency: u8,
    pub delay: u16,
    /*TODO: USB 2 and 3 disagree on the descriptor, so some fields are disabled
    // device_removable: bitmap of ports, maximum of 256 bits (32 bytes)
    // power_control_mask: bitmap of ports, maximum of 256 bits (32 bytes)
    bitmaps: [u8; 64],
    */
}

unsafe impl plain::Plain for HubDescriptorV3 {}

impl HubDescriptorV3 {
    pub const DESCRIPTOR_KIND: u8 = 0x2A;
}

impl Default for HubDescriptorV3 {
    fn default() -> Self {
        Self {
            length: 0,
            kind: 0,
            ports: 0,
            characteristics: 0,
            power_on_good: 0,
            current: 0,
            decode_latency: 0,
            delay: 0,
            /*
            bitmaps: [0; 64],
            */
        }
    }
}

// This only includes matching features from both USB 2.0 and 3.0 specs
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum HubPortFeature {
    PortConnection = 0,
    PortOverCurrent = 3,
    PortReset = 4,
    PortLinkState = 5,
    PortPower = 8,
    CPortConnection = 16,
    CPortOverCurrent = 19,
    CPortReset = 20,
}

bitflags::bitflags! {
    #[derive(Default)]
    #[repr(transparent)]
    pub struct HubPortStatusV2: u32 {
        const CONNECTION = 1 << 0;
        const ENABLE = 1 << 1;
        const SUSPEND = 1 << 2;
        const OVER_CURRENT = 1 << 3;
        const RESET = 1 << 4;
        // bits 5-7 reserved
        const POWER = 1 << 8;
        const LOW_SPEED = 1 << 9;
        const HIGH_SPEED = 1 << 10;
        const TEST = 1 << 11;
        const INDICATOR = 1 << 12;
        // bits 13-15 reserved
        const CONNECTION_CHANGED = 1 << 16;
        const ENABLE_CHANGED = 1 << 17;
        const SUSPEND_CHANGED = 1 << 18;
        const OVER_CURRENT_CHANGED = 1 << 19;
        const RESET_CHANGED = 1 << 20;
        // bits 21 - 31 reserved
    }
}

unsafe impl plain::Plain for HubPortStatusV2 {}

bitflags::bitflags! {
    #[derive(Default)]
    #[repr(transparent)]
    pub struct HubPortStatusV3: u32 {
        const CONNECTION = 1 << 0;
        const ENABLE = 1 << 1;
        // bit 2 reserved
        const OVER_CURRENT = 1 << 3;
        const RESET = 1 << 4;
        const LINK_STATE_0 = 1 << 5;
        const LINK_STATE_1 = 1 << 6;
        const LINK_STATE_2 = 1 << 7;
        const LINK_STATE_3 = 1 << 8;
        const POWER = 1 << 9;
        const SPEED_0 = 1 << 10;
        const SPEED_1 = 1 << 11;
        const SPEED_2 = 1 << 12;
        // bits 13 - 15 reserved
        const CONNECTION_CHANGED = 1 << 16;
        // bits 17-18
        const OVER_CURRENT_CHANGED = 1 << 19;
        const RESET_CHANGED = 1 << 20;
        const BH_RESET_CHANGED = 1 << 21;
        const LINK_STATE_CHANGED = 1 << 22;
        const CONFIG_ERROR = 1 << 23;
        // bits 24 - 31 reserved
    }
}

unsafe impl plain::Plain for HubPortStatusV3 {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HubPortStatus {
    V2(HubPortStatusV2),
    V3(HubPortStatusV3),
}

impl HubPortStatus {
    pub fn is_powered(&self) -> bool {
        match self {
            Self::V2(x) => x.contains(HubPortStatusV2::POWER),
            Self::V3(x) => x.contains(HubPortStatusV3::POWER),
        }
    }

    pub fn is_connected(&self) -> bool {
        match self {
            Self::V2(x) => x.contains(HubPortStatusV2::CONNECTION),
            Self::V3(x) => x.contains(HubPortStatusV3::CONNECTION),
        }
    }

    pub fn is_resetting(&self) -> bool {
        match self {
            Self::V2(x) => x.contains(HubPortStatusV2::RESET),
            Self::V3(x) => x.contains(HubPortStatusV3::RESET),
        }
    }

    pub fn is_enabled(&self) -> bool {
        match self {
            Self::V2(x) => x.contains(HubPortStatusV2::ENABLE),
            Self::V3(x) => x.contains(HubPortStatusV3::ENABLE),
        }
    }
}
