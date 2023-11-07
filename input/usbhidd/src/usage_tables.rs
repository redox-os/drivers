// See https://www.usb.org/sites/default/files/documents/hut1_12v2.pdf

#[repr(u8)]
pub enum UsagePage {
    GenericDesktop = 1,
    SimulationsControl,
    VrControls,
    SportControls,
    GameControls,
    GenericDeviceControls,
    KeyboardOrKeypad,
    Led,
    Button,
    Ordinal,
    TelephonyDevice,
    Consumer,
    Digitizer,
    Unicode = 0x10,
    AlphanumericDisplay = 0x14,
    MedicalInstrument = 0x40,
}

#[repr(u8)]
pub enum GenericDesktopUsage {
    Pointer = 0x01,
    Mouse,
    Joystick = 0x04,
    GamePad,
    Keyboard,
    Keypad,
    MultiAxisController,

    // 0x0A-0x2F are reserved

    CountedBuffer = 0x3A,
    SysControl = 0x80,
}

#[repr(u8)]
pub enum KeyboardOrKeypadUsage {
    KbdErrorRollover = 0x1,
    KbdPostFail,
    KbdErrorUndefined,
    // the rest are used as regular keycodes
}
