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
    bitmaps: [u8; 64],
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
            bitmaps: [0; 64],
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum HubFeature {
    //TODO: CHubLocalPower = 0,
    //TODO: CHubOverCurrent = 1,
    PortConnection = 0,
    PortEnable = 1,
    PortSuspend = 2,
    PortOverCurrent = 3,
    PortReset = 4,
    PortPower = 8,
    PortLowSpeed = 9,
    CPortConnection = 16,
    CPortEnable = 17,
    CPortSuspend = 18,
    CPortOverCurrent = 19,
    CPortReset = 20,
    PortTest = 21,
    PortIndicator = 22,
}

bitflags::bitflags! {
    #[derive(Default)]
    #[repr(transparent)]
    pub struct HubPortStatus: u32 {
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

unsafe impl plain::Plain for HubPortStatus {}
