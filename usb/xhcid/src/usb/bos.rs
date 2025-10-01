use std::slice;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BosDescriptor {
    pub len: u8,
    pub kind: u8,
    pub total_len: u16,
    pub cap_count: u8,
}

unsafe impl plain::Plain for BosDescriptor {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BosDevDescriptorBase {
    pub len: u8,
    pub kind: u8,
    pub cap_ty: u8,
}

unsafe impl plain::Plain for BosDevDescriptorBase {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BosSuperSpeedDesc {
    pub len: u8,
    pub kind: u8,
    pub cap_ty: u8,

    pub attrs: u8,
    pub speed_supp: u16,
    pub func_supp: u8,
    pub u1_dev_exit_lat: u8,
    pub u2_dev_exit_lat: u16,
}
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BosSuperSpeedPlusDesc {
    pub len: u8,
    pub kind: u8,
    pub cap_ty: u8,
    pub _rsvd0: u8,
    pub attrs: u32,
    pub func_supp: u32,
    pub _rsvd1: u16,
}

unsafe impl plain::Plain for BosSuperSpeedPlusDesc {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BosUsb2ExtDesc {
    pub len: u8,
    pub kind: u8,
    pub cap_ty: u8,

    pub attrs: u32,
}

unsafe impl plain::Plain for BosUsb2ExtDesc {}

#[repr(u8)]
pub enum DeviceCapability {
    Usb2Ext = 0x02,
    SuperSpeed,
    SuperSpeedPlus = 0x0A,
}

unsafe impl plain::Plain for BosSuperSpeedDesc {}

impl BosSuperSpeedPlusDesc {
    pub fn ssac(&self) -> u8 {
        (self.attrs & 0x0000_000F) as u8
    }
    pub fn sublink_speed_attr(&self) -> &[u32] {
        unsafe {
            slice::from_raw_parts(
                (self as *const Self).add(1) as *const u32,
                self.ssac() as usize + 1,
            )
        }
    }
}

pub struct BosDevDescIter<'a> {
    bytes: &'a [u8],
}
impl<'a> BosDevDescIter<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}
impl<'a> From<&'a [u8]> for BosDevDescIter<'a> {
    fn from(slice: &'a [u8]) -> Self {
        Self::new(slice)
    }
}
impl<'a> Iterator for BosDevDescIter<'a> {
    type Item = (BosDevDescriptorBase, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(desc) = plain::from_bytes::<BosDevDescriptorBase>(self.bytes).ok() {
            if desc.len as usize > self.bytes.len() {
                return None;
            };
            let bytes_ret = &self.bytes[..desc.len as usize];
            self.bytes = &self.bytes[desc.len as usize..];
            Some((*desc, bytes_ret))
        } else {
            return None;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BosAnyDevDesc {
    Usb2Ext(BosUsb2ExtDesc),
    SuperSpeed(BosSuperSpeedDesc),
    SuperSpeedPlus(BosSuperSpeedPlusDesc),
    Unknown,
}

impl BosAnyDevDesc {
    pub fn is_superspeed(&self) -> bool {
        match self {
            Self::SuperSpeed(_) => true,
            _ => false,
        }
    }
    pub fn is_superspeedplus(&self) -> bool {
        match self {
            Self::SuperSpeedPlus(_) => true,
            _ => false,
        }
    }
}

pub struct BosAnyDevDescIter<'a> {
    inner: BosDevDescIter<'a>,
}
impl<'a> From<BosDevDescIter<'a>> for BosAnyDevDescIter<'a> {
    fn from(ll: BosDevDescIter<'a>) -> Self {
        Self { inner: ll }
    }
}
impl<'a> From<&'a [u8]> for BosAnyDevDescIter<'a> {
    fn from(slice: &'a [u8]) -> Self {
        Self::from(BosDevDescIter::from(slice))
    }
}
impl<'a> Iterator for BosAnyDevDescIter<'a> {
    type Item = BosAnyDevDesc;

    fn next(&mut self) -> Option<Self::Item> {
        let (base, slice) = self.inner.next()?;

        if base.cap_ty == DeviceCapability::Usb2Ext as u8 {
            Some(BosAnyDevDesc::Usb2Ext(*plain::from_bytes(slice).ok()?))
        } else if base.cap_ty == DeviceCapability::SuperSpeed as u8 {
            Some(BosAnyDevDesc::SuperSpeed(*plain::from_bytes(slice).ok()?))
        } else if base.cap_ty == DeviceCapability::SuperSpeedPlus as u8 {
            Some(BosAnyDevDesc::SuperSpeedPlus(
                *plain::from_bytes(slice).ok()?,
            ))
        } else if base.cap_ty == 0 {
            // TODO
            return None;
        } else {
            log::warn!("unknown USB device capability of type: {:#x}", base.cap_ty);
            Some(BosAnyDevDesc::Unknown)
        }
    }
}

pub fn bos_capability_descs<'a>(
    desc: BosDescriptor,
    data: &'a [u8],
) -> impl Iterator<Item = BosAnyDevDesc> + 'a {
    BosAnyDevDescIter::from(&data[..desc.total_len as usize - std::mem::size_of_val(&desc)])
        .take(desc.cap_count as usize)
}
