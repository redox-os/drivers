use common::io::{Io, Mmio};
use std::ops::Range;
use std::ptr::NonNull;
use std::{fmt, mem, ptr, slice};

pub struct ExtendedCapabilitiesIter {
    base: *const u8,
}
impl ExtendedCapabilitiesIter {
    pub unsafe fn new(base: *const u8) -> Self {
        Self { base }
    }
}
impl Iterator for ExtendedCapabilitiesIter {
    type Item = (NonNull<u8>, u8); // pointer, capability id

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let current = NonNull::new(self.base as *mut _)?;

            let reg = current.cast::<Mmio<u32>>().as_ref().read();
            let capability_id = (reg & 0xFF) as u8;
            let next_rel_in_dwords = ((reg & 0xFF00) >> 8) as u8;

            let next_rel = u16::from(next_rel_in_dwords) << 2;

            self.base = if next_rel != 0 {
                self.base.offset(next_rel as isize)
            } else {
                ptr::null()
            };

            Some((current, capability_id))
        }
    }
}

#[repr(u8)]
pub enum CapabilityId {
    // bit 0 is reserved
    UsbLegacySupport = 1,
    SupportedProtocol,
    ExtendedPowerManagement,
    IoVirtualization,
    MessageInterrupt,
    LocalMem,
    // bits 7-9 are reserved
    UsbDebugCapability = 10,
    // bits 11-16 are reserved
    ExtendedMessageInterrupt = 17,
    // bits 18-191 are reserved
    // bits 192-255 are vendor-defined
}

#[repr(C, packed)]
pub struct SupportedProtoCap {
    a: Mmio<u32>,
    b: Mmio<u32>,
    c: Mmio<u32>,
    d: Mmio<u32>,
    protocol_speeds: [u8; 0],
}

#[repr(C, packed)]
pub struct ProtocolSpeed {
    a: Mmio<u32>,
}

pub const PROTO_SPEED_PSIV_MASK: u32 = 0x0000_000F;
pub const PROTO_SPEED_PSIV_SHIFT: u8 = 0;

pub const PROTO_SPEED_PSIE_MASK: u32 = 0x0000_0030;
pub const PROTO_SPEED_PSIE_SHIFT: u8 = 4;

pub const PROTO_SPEED_PLT_MASK: u32 = 0x0000_00C0;
pub const PROTO_SPEED_PLT_SHIFT: u8 = 6;

pub const PROTO_SPEED_PFD_BIT: u32 = 1 << PROTO_SPEED_PFD_SHIFT;
pub const PROTO_SPEED_PFD_SHIFT: u8 = 8;

pub const PROTO_SPEED_LP_MASK: u32 = 0x0000_C000;
pub const PROTO_SPEED_LP_SHIFT: u8 = 14;

pub const PROTO_SPEED_PSIM_MASK: u32 = 0xFFFF_0000;
pub const PROTO_SPEED_PSIM_SHIFT: u8 = 16;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Psie {
    Bps,
    Kbps,
    Mbps,
    Gbps,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Plt {
    Symmetric,
    Reserved,
    AsymmetricRx,
    AsymmetricTx,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Lp {
    SuperSpeed,
    SuperSpeedPlus,
    Rsvd2,
    Rsvd3,
}

impl ProtocolSpeed {
    pub const fn from_raw(raw: u32) -> Self {
        Self { a: Mmio::new(raw) }
    }
    pub fn is_lowspeed(&self) -> bool {
        self.psim() == 1500 && self.psie() == Psie::Kbps && !self.pfd()
    }
    pub fn is_fullspeed(&self) -> bool {
        self.psim() == 12 && self.psie() == Psie::Mbps && !self.pfd()
    }
    pub fn is_highspeed(&self) -> bool {
        self.psim() == 480 && self.psie() == Psie::Mbps && !self.pfd()
    }
    pub fn is_superspeed_gen1x1(&self) -> bool {
        self.psim() == 5 && self.psie() == Psie::Gbps && self.pfd() && self.lp() == Lp::SuperSpeed
    }
    pub fn is_superspeedplus_gen2x1(&self) -> bool {
        self.psim() == 10
            && self.psie() == Psie::Gbps
            && self.pfd()
            && self.lp() == Lp::SuperSpeedPlus
    }
    pub fn is_superspeedplus_gen1x2(&self) -> bool {
        self.psim() == 10
            && self.psie() == Psie::Gbps
            && self.pfd()
            && self.lp() == Lp::SuperSpeedPlus
    }
    pub fn is_superspeedplus_gen2x2(&self) -> bool {
        self.psim() == 20
            && self.psie() == Psie::Gbps
            && self.pfd()
            && self.lp() == Lp::SuperSpeedPlus
    }
    pub fn is_superspeed_gen_x(&self) -> bool {
        self.is_superspeed_gen1x1()
            || self.is_superspeedplus_gen2x1()
            || self.is_superspeedplus_gen1x2()
            || self.is_superspeedplus_gen2x2()
    }
    /// Protocol speed ID value
    pub fn psiv(&self) -> u8 {
        ((self.a.read() & PROTO_SPEED_PSIV_MASK) >> PROTO_SPEED_PSIV_SHIFT) as u8
    }
    pub fn psie_raw(&self) -> u8 {
        ((self.a.read() & PROTO_SPEED_PSIE_MASK) >> PROTO_SPEED_PSIE_SHIFT) as u8
    }
    /// Protocol speed ID exponent
    pub fn psie(&self) -> Psie {
        // safe because psie_raw can only return values in 0..=3
        unsafe { mem::transmute(self.psie_raw()) }
    }
    pub fn plt_raw(&self) -> u8 {
        ((self.a.read() & PROTO_SPEED_PLT_MASK) >> PROTO_SPEED_PLT_SHIFT) as u8
    }
    /// PSI type
    pub fn plt(&self) -> Plt {
        // safe because plt_raw can only return values in 0..=3
        unsafe { mem::transmute(self.plt_raw()) }
    }
    /// PSI Full-duplex
    pub fn pfd(&self) -> bool {
        self.a.readf(PROTO_SPEED_PFD_BIT)
    }
    pub fn lp_raw(&self) -> u8 {
        ((self.a.read() & PROTO_SPEED_LP_MASK) >> PROTO_SPEED_LP_SHIFT) as u8
    }
    /// Link protocol
    pub fn lp(&self) -> Lp {
        // safe because lp_raw can only return values in 0..=3
        unsafe { mem::transmute(self.lp_raw()) }
    }
    /// Protocol speed ID mantissa
    pub fn psim(&self) -> u16 {
        ((self.a.read() & PROTO_SPEED_PSIM_MASK) >> PROTO_SPEED_PSIM_SHIFT) as u16
    }
}

impl fmt::Debug for ProtocolSpeed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtocolSpeed")
            .field("psiv", &self.psiv())
            .field("psie", &self.psie())
            .field("plt", &self.plt())
            .field("pfd", &self.pfd())
            .field("lp", &self.lp())
            .field("psim", &self.psim())
            .finish()
    }
}

pub const SUPP_PROTO_CAP_REV_MIN_MASK: u32 = 0x00FF_0000;
pub const SUPP_PROTO_CAP_REV_MIN_SHIFT: u8 = 16;

pub const SUPP_PROTO_CAP_REV_MAJ_MASK: u32 = 0xFF00_0000;
pub const SUPP_PROTO_CAP_REV_MAJ_SHIFT: u8 = 24;

pub const SUPP_PROTO_CAP_COMPAT_PORT_OFF_MASK: u32 = 0x0000_00FF;
pub const SUPP_PROTO_CAP_COMPAT_PORT_OFF_SHIFT: u8 = 0;

pub const SUPP_PROTO_CAP_COMPAT_PORT_CNT_MASK: u32 = 0x0000_FF00;
pub const SUPP_PROTO_CAP_COMPAT_PORT_CNT_SHIFT: u8 = 8;

pub const SUPP_PROTO_CAP_PROTO_DEF_MASK: u32 = 0x0FFF_0000;
pub const SUPP_PROTO_CAP_PROTO_DEF_SHIFT: u8 = 16;

pub const SUPP_PROTO_CAP_PSIC_MASK: u32 = 0xF000_0000;
pub const SUPP_PROTO_CAP_PSIC_SHIFT: u8 = 28;

pub const SUPP_PROTO_CAP_PORT_SLOT_TYPE_MASK: u32 = 0x0000_001F;
pub const SUPP_PROTO_CAP_PORT_SLOT_TYPE_SHIFT: u8 = 0;

impl SupportedProtoCap {
    pub unsafe fn protocol_speeds(&self) -> &[ProtocolSpeed] {
        slice::from_raw_parts(
            &self.protocol_speeds as *const u8 as *const _,
            self.psic() as usize,
        )
    }
    pub unsafe fn protocol_speeds_mut(&mut self) -> &mut [ProtocolSpeed] {
        // XXX: Variance really is annoying sometimes.
        slice::from_raw_parts_mut(
            &self.protocol_speeds as *const u8 as *mut u8 as *mut _,
            self.psic() as usize,
        )
    }
    pub fn rev_minor(&self) -> u8 {
        ((self.a.read() & SUPP_PROTO_CAP_REV_MIN_MASK) >> SUPP_PROTO_CAP_REV_MIN_SHIFT) as u8
    }
    pub fn rev_major(&self) -> u8 {
        ((self.a.read() & SUPP_PROTO_CAP_REV_MAJ_MASK) >> SUPP_PROTO_CAP_REV_MAJ_SHIFT) as u8
    }
    pub fn name_string(&self) -> [u8; 4] {
        // TODO: Little endian, right?
        u32::to_le_bytes(self.b.read())
    }
    pub fn compat_port_offset(&self) -> u8 {
        ((self.c.read() & SUPP_PROTO_CAP_COMPAT_PORT_OFF_MASK)
            >> SUPP_PROTO_CAP_COMPAT_PORT_OFF_SHIFT) as u8
    }
    pub fn compat_port_count(&self) -> u8 {
        ((self.c.read() & SUPP_PROTO_CAP_COMPAT_PORT_CNT_MASK)
            >> SUPP_PROTO_CAP_COMPAT_PORT_CNT_SHIFT) as u8
    }
    pub fn compat_port_range(&self) -> Range<u8> {
        self.compat_port_offset()..self.compat_port_offset() + self.compat_port_count()
    }

    pub fn proto_defined(&self) -> u16 {
        ((self.c.read() & SUPP_PROTO_CAP_PROTO_DEF_MASK) >> SUPP_PROTO_CAP_PROTO_DEF_SHIFT) as u16
    }
    pub fn psic(&self) -> u8 {
        ((self.c.read() & SUPP_PROTO_CAP_PSIC_MASK) >> SUPP_PROTO_CAP_PSIC_SHIFT) as u8
    }
    pub fn proto_slot_ty(&self) -> u8 {
        ((self.d.read() & SUPP_PROTO_CAP_PORT_SLOT_TYPE_MASK)
            >> SUPP_PROTO_CAP_PORT_SLOT_TYPE_SHIFT) as u8
    }
}
impl fmt::Debug for SupportedProtoCap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SupportedProtoCap")
            .field("rev_minor", &self.rev_minor())
            .field("rev_major", &self.rev_major())
            .field("name_string", &String::from_utf8_lossy(&self.name_string()))
            .field("compat_port_offset", &self.compat_port_count())
            .field("compat_port_count", &self.compat_port_offset())
            .field("proto_defined", &self.proto_defined())
            .field("psic", &self.psic())
            .field("proto_slot_ty", &self.proto_slot_ty())
            .field("proto_speeds", unsafe {
                &self.protocol_speeds().to_owned()
            })
            .finish()
    }
}
