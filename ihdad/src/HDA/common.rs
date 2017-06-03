

use std::{mem, thread, ptr, fmt};

pub type HDANodeAddr  = u16;
pub type HDACodecAddr = u16;

#[derive(Debug, PartialEq)]
#[repr(u8)]
pub enum HDAWidgetType{
	AudioOutput   = 0x0,
	AudioInput    = 0x1,
	AudioMixer    = 0x2,
	AudioSelector = 0x3,
	PinComplex    = 0x4,
	Power         = 0x5,
	VolumeKnob    = 0x6,
	BeepGenerator = 0x7,
	

	VendorDefined = 0xf,
}

impl fmt::Display for HDAWidgetType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}


#[derive(Debug)]
#[repr(u8)]
pub enum HDADefaultDevice {
	LineOut          = 0x0,
	Speaker          = 0x1,
	HPOut            = 0x2,
	CD               = 0x3,
	SPDIF            = 0x4,
	DigitalOtherOut  = 0x5,
	ModemLineSide    = 0x6,
	ModemHandsetSide = 0x7,
	LineIn           = 0x8,
	AUX              = 0x9,
	MicIn            = 0xA,
	Telephony        = 0xB,
	SPDIFIn          = 0xC,
	DigitalOtherIn   = 0xD,
	Reserved         = 0xE,
	Other            = 0xF,
}

