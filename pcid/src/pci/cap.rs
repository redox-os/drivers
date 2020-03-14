use super::func::{ConfigReader, ConfigWriter};

pub struct CapabilityOffsetsIter<'a, R> {
    offset: u8,
    reader: &'a R,
}
impl<'a, R> CapabilityOffsetsIter<'a, R> {
    pub fn new(offset: u8, reader: &'a R) -> Self {
        Self {
            offset,
            reader,
        }
    }
}
impl<'a, R> Iterator for CapabilityOffsetsIter<'a, R>
where
    R: ConfigReader
{
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            assert_eq!(self.offset & 0xF8, self.offset, "capability must be dword aligned");

            if self.offset == 0 { return None };

            let first_dword = dbg!(self.reader.read_u32(dbg!(self.offset)));
            let next = ((first_dword >> 8) & 0xFF) as u8;

            let offset = self.offset;
            self.offset = next;

            Some(offset)
        }
    }
}

#[repr(u8)]
pub enum CapabilityId {
    Msi = 0x05,
    MsiX = 0x11,
    Pcie = 0x10,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MsiCapability {
    _32BitAddress {
        message_control: u32,
        message_address: u32,
        message_data: u32,
    },
    _64BitAddress {
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u32,
    },
    _32BitAddressWithPvm {
        message_control: u32,
        message_address: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
    _64BitAddressWithPvm {
        message_control: u32,
        message_address_lo: u32,
        message_address_hi: u32,
        message_data: u32,
        mask_bits: u32,
        pending_bits: u32,
    },
}

impl MsiCapability {
    pub const MC_PVT_CAPABLE_BIT: u16 = 1 << 8;
    pub const MC_64_BIT_ADDR_BIT: u16 = 1 << 7;

    pub const MC_MULTI_MESSAGE_MASK: u16 = 0x000E;
    pub const MC_MULTI_MESSAGE_SHIFT: u8 = 1;

    pub const MC_MULTI_MESSAGE_ENABLE_MASK: u16 = 0x0070;
    pub const MC_MULTI_MESSAGE_ENABLE_SHIFT: u8 = 4;

    pub const MC_MSI_ENABLED_BIT: u16 = 1;

    pub unsafe fn parse<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        let dword = reader.read_u32(offset);

        let message_control = (dword >> 16) as u16;

        if message_control & Self::MC_PVT_CAPABLE_BIT != 0 {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddressWithPvm {
                    message_control: dword,
                    message_address_lo: reader.read_u32(offset + 4),
                    message_address_hi: reader.read_u32(offset + 8),
                    message_data: reader.read_u32(offset + 12),
                    mask_bits: reader.read_u32(offset + 16),
                    pending_bits: reader.read_u32(offset + 20),
                }
            } else {
                Self::_32BitAddressWithPvm {
                    message_control: dword,
                    message_address: reader.read_u32(offset + 4),
                    message_data: reader.read_u32(offset + 8),
                    mask_bits: reader.read_u32(offset + 12),
                    pending_bits: reader.read_u32(offset + 16),
                }
            }
        } else {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddress {
                    message_control: dword,
                    message_address_lo: reader.read_u32(offset + 4),
                    message_address_hi: reader.read_u32(offset + 8),
                    message_data: reader.read_u32(offset + 12),
                }
            } else {
                Self::_32BitAddress {
                    message_control: dword,
                    message_address: reader.read_u32(offset + 4),
                    message_data: reader.read_u32(offset + 8),
                }
            }
        }
    }

    fn message_control_raw(&self) -> u32 {
        match self {
            Self::_32BitAddress { message_control, .. } | Self::_64BitAddress { message_control, .. } | Self::_32BitAddressWithPvm { message_control, .. } | Self::_64BitAddressWithPvm { message_control, .. } => *message_control,
        }
    }
    pub fn message_control(&self) -> u16 {
        self.message_control_raw() as u16
    }
    pub unsafe fn set_message_control<W: ConfigWriter>(&mut self, writer: &mut W, offset: u8, value: u16) {
        let mut new_message_control = self.message_control_raw();
        new_message_control &= 0x0000_FFFF;
        new_message_control |= u32::from(value) << 16;
        writer.write_u32(offset, new_message_control);

        match self {
            Self::_32BitAddress { ref mut message_control, .. }
                | Self::_64BitAddress { ref mut message_control, .. }
                | Self::_32BitAddressWithPvm { ref mut message_control, .. }
                | Self::_64BitAddressWithPvm { ref mut message_control, .. } => *message_control = new_message_control,
        }
    }
    pub fn is_pvt_capable(&self) -> bool {
        self.message_control() & Self::MC_PVT_CAPABLE_BIT != 0
    }
    pub fn has_64_bit_addr(&self) -> bool {
        self.message_control() & Self::MC_64_BIT_ADDR_BIT != 0
    }
    pub fn enabled(&self) -> bool {
        self.message_control() & Self::MC_MSI_ENABLED_BIT != 0
    }
    pub unsafe fn set_enabled<W: ConfigWriter>(&mut self, writer: &mut W, offset: u8, enabled: bool) {
        let mut new_message_control = self.message_control() & (!Self::MC_MSI_ENABLED_BIT);
        new_message_control |= u16::from(enabled);
        self.set_message_control(writer, offset, new_message_control)
    }
    pub fn multi_message_capable(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_MASK) >> Self::MC_MULTI_MESSAGE_SHIFT) as u8
    }
    pub fn multi_message_enabled(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_ENABLE_MASK) >> Self::MC_MULTI_MESSAGE_ENABLE_MASK) as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PcieCapability {
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MsixCapability {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl MsixCapability {
    pub const MC_MSIX_ENABLED_BIT: u16 = 1 << 15;
    pub const MC_MSIX_ENABLED_SHIFT: u8 = 15;
    pub const MC_FUNCTION_MASK_BIT: u16 = 1 << 14;
    pub const MC_FUNCTION_MASK_SHIFT: u8 = 14;
    pub const MC_TABLE_SIZE_MASK: u16 = 0x03FF;

    /// The Message Control field, containing the enabled and function mask bits, as well as the
    /// table size.
    pub const fn message_control(&self) -> u16 {
        (self.a >> 16) as u16
    }
    pub fn set_message_control(&mut self, message_control: u16) {
        self.a &= 0x0000_FFFF;
        self.a |= u32::from(message_control) << 16;
    }
    /// Returns the MSI-X table size, subtracted by one.
    pub const fn table_size_raw(&self) -> u16 {
        self.message_control() & Self::MC_TABLE_SIZE_MASK
    }
    /// Returns the MSI-X table size.
    pub const fn table_size(&self) -> u16 {
        self.table_size_raw() + 1
    }
    /// Returns the MSI-X enabled bit, which enables MSI-X if the MSI enable bit is also set in the
    /// MSI capability structure.
    pub const fn msix_enabled(&self) -> bool {
        self.message_control() & Self::MC_MSIX_ENABLED_BIT != 0
    }
    /// The MSI-X function mask, which overrides each of the vectors' mask bit, when set.
    pub const fn function_mask(&self) -> bool {
        self.message_control() & Self::MC_FUNCTION_MASK_BIT != 0
    }

    pub fn set_msix_enabled(&mut self, enabled: bool) {
        let mut new_message_control = self.message_control();
        new_message_control &= !(Self::MC_MSIX_ENABLED_BIT);
        new_message_control |= u16::from(enabled) << Self::MC_MSIX_ENABLED_SHIFT;
        self.set_message_control(new_message_control);
    }

    pub fn set_function_mask(&mut self, function_mask: bool) {
        let mut new_message_control = self.message_control();
        new_message_control &= !(Self::MC_FUNCTION_MASK_BIT);
        new_message_control |= u16::from(function_mask) << Self::MC_FUNCTION_MASK_SHIFT;
        self.set_message_control(new_message_control);
    }
    pub const TABLE_OFFSET_MASK: u32 = 0xFFFF_FFF8;
    pub const TABLE_BIR_MASK: u32 = 0x0000_0007;

    /// The table offset is guaranteed to be QWORD aligned (8 bytes).
    pub const fn table_offset(&self) -> u32 {
        self.b & Self::TABLE_OFFSET_MASK
    }
    /// The table BIR, which is used to map the offset to a memory location.
    pub const fn table_bir(&self) -> u8 {
        (self.b & Self::TABLE_BIR_MASK) as u8
    }

    pub fn set_table_offset(&mut self, offset: u32) {
        assert_eq!(offset & Self::TABLE_OFFSET_MASK, offset, "MSI-X table offset has to be QWORD aligned");
        self.b &= !Self::TABLE_OFFSET_MASK;
        self.b |= offset;
    }
    pub const PBA_OFFSET_MASK: u32 = 0xFFFF_FFF8;
    pub const PBA_BIR_MASK: u32 = 0x0000_0007;

    /// The Pending Bit Array offset is guaranteed to be QWORD aligned (8 bytes).
    pub const fn pba_offset(&self) -> u32 {
        self.b & Self::PBA_OFFSET_MASK
    }
    /// The Pending Bit Array BIR, which is used to map the offset to a memory location.
    pub const fn pba_bir(&self) -> u8 {
        (self.b & Self::PBA_BIR_MASK) as u8
    }

    pub fn set_pba_offset(&mut self, offset: u32) {
        assert_eq!(offset & Self::PBA_OFFSET_MASK, offset, "MSI-X Pending Bit Array offset has to be QWORD aligned");
        self.c &= !Self::PBA_OFFSET_MASK;
        self.c |= offset;
    }

    /// Write the first DWORD into configuration space (containing the partially modifiable Message
    /// Control field).
    pub unsafe fn write_a<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(offset, self.a)
    }
    /// Write the second DWORD into configuration space (containing the modifiable table
    /// offset and the readonly table BIR).
    pub unsafe fn write_b<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(offset + 4, self.a)
    }
    /// Write the third DWORD into configuration space (containing the modifiable pending bit array
    /// offset, and the readonly PBA BIR).
    pub unsafe fn write_c<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(offset + 8, self.a)
    }
    /// Write this capability structure back to configuration space.
    pub unsafe fn write_all<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        self.write_a(writer, offset);
        self.write_b(writer, offset);
        self.write_c(writer, offset);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Capability {
    Msi(MsiCapability),
    MsiX(MsixCapability),
    Pcie(PcieCapability),
    Other(u8),
}

impl Capability {
    unsafe fn parse_msi<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::Msi(MsiCapability::parse(reader, offset))
    }
    unsafe fn parse_msix<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        Self::MsiX(MsixCapability {
            a: reader.read_u32(offset),
            b: reader.read_u32(offset + 4),
            c: reader.read_u32(offset + 8),
        })
    }
    unsafe fn parse_pcie<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        // TODO
        Self::Pcie(PcieCapability {})
    }
    unsafe fn parse<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        assert_eq!(offset & 0xF8, offset, "capability must be dword aligned");

        let dword = reader.read_u32(offset);
        let capability_id = (dword & 0xFF) as u8;

        if capability_id == CapabilityId::Msi as u8 {
            Self::parse_msi(reader, offset)
        } else if capability_id == CapabilityId::MsiX as u8 {
            Self::parse_msix(reader, offset)
        } else if capability_id == CapabilityId::Pcie as u8 {
            Self::parse_pcie(reader, offset)
        } else {
            Self::Other(capability_id)
            //panic!("unimplemented or malformed capability id: {}", capability_id)
        }
    }
}

pub struct CapabilitiesIter<'a, R> {
    pub inner: CapabilityOffsetsIter<'a, R>,
}

impl<'a, R> Iterator for CapabilitiesIter<'a, R>
where
    R: ConfigReader
{
    type Item = Capability;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.inner.next()?;
        Some(unsafe { Capability::parse(self.inner.reader, offset) })
    }
}
