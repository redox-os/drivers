use std::fmt;

use super::bar::PciBar;
pub use super::cap::{MsiCapability, MsixCapability};
use super::func::{ConfigReader, ConfigWriter};

use syscall::{Io, Mmio};

impl MsiCapability {
    pub const MC_PVT_CAPABLE_BIT: u16 = 1 << 8;
    pub const MC_64_BIT_ADDR_BIT: u16 = 1 << 7;

    pub const MC_MULTI_MESSAGE_MASK: u16 = 0x000E;
    pub const MC_MULTI_MESSAGE_SHIFT: u8 = 1;

    pub const MC_MULTI_MESSAGE_ENABLE_MASK: u16 = 0x0070;
    pub const MC_MULTI_MESSAGE_ENABLE_SHIFT: u8 = 4;

    pub const MC_MSI_ENABLED_BIT: u16 = 1;

    pub unsafe fn parse<R: ConfigReader>(reader: &R, offset: u8) -> Self {
        let dword = reader.read_u32(u16::from(offset));

        let message_control = (dword >> 16) as u16;

        if message_control & Self::MC_PVT_CAPABLE_BIT != 0 {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddressWithPvm {
                    message_control: dword,
                    message_address_lo: reader.read_u32(u16::from(offset + 4)),
                    message_address_hi: reader.read_u32(u16::from(offset + 8)),
                    message_data: reader.read_u32(u16::from(offset + 12)),
                    mask_bits: reader.read_u32(u16::from(offset + 16)),
                    pending_bits: reader.read_u32(u16::from(offset + 20)),
                }
            } else {
                Self::_32BitAddressWithPvm {
                    message_control: dword,
                    message_address: reader.read_u32(u16::from(offset + 4)),
                    message_data: reader.read_u32(u16::from(offset + 8)),
                    mask_bits: reader.read_u32(u16::from(offset + 12)),
                    pending_bits: reader.read_u32(u16::from(offset + 16)),
                }
            }
        } else {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddress {
                    message_control: dword,
                    message_address_lo: reader.read_u32(u16::from(offset + 4)),
                    message_address_hi: reader.read_u32(u16::from(offset + 8)),
                    message_data: reader.read_u32(u16::from(offset + 12)) as u16,
                }
            } else {
                Self::_32BitAddress {
                    message_control: dword,
                    message_address: reader.read_u32(u16::from(offset + 4)),
                    message_data: reader.read_u32(u16::from(offset + 8)) as u16,
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
        (self.message_control_raw() >> 16) as u16
    }
    pub fn set_message_control(&mut self, value: u16) {
        let mut new_message_control = self.message_control_raw();
        new_message_control &= 0x0000_FFFF;
        new_message_control |= u32::from(value) << 16;

        match self {
            Self::_32BitAddress { ref mut message_control, .. }
                | Self::_64BitAddress { ref mut message_control, .. }
                | Self::_32BitAddressWithPvm { ref mut message_control, .. }
                | Self::_64BitAddressWithPvm { ref mut message_control, .. } => *message_control = new_message_control,
        }
    }
    pub unsafe fn write_message_control<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(u16::from(offset), self.message_control_raw());
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
    pub fn set_enabled(&mut self, enabled: bool) {
        let mut new_message_control = self.message_control() & (!Self::MC_MSI_ENABLED_BIT);
        new_message_control |= u16::from(enabled);
        self.set_message_control(new_message_control);
    }
    pub fn multi_message_capable(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_MASK) >> Self::MC_MULTI_MESSAGE_SHIFT) as u8
    }
    pub fn multi_message_enable(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_ENABLE_MASK) >> Self::MC_MULTI_MESSAGE_ENABLE_SHIFT) as u8
    }
    pub fn set_multi_message_enable(&mut self, log_mme: u8) {
        let mut new_message_control = self.message_control() & (!Self::MC_MULTI_MESSAGE_ENABLE_MASK);
        new_message_control |= (u16::from(log_mme) << Self::MC_MULTI_MESSAGE_ENABLE_SHIFT);
        self.set_message_control(new_message_control);
    }

    pub fn message_address(&self) -> u32 {
        match self {
            &Self::_32BitAddress { message_address, .. } | &Self::_32BitAddressWithPvm { message_address, .. } => message_address,
            &Self::_64BitAddress { message_address_lo, .. } | &Self::_64BitAddressWithPvm { message_address_lo, .. } => message_address_lo,
        }
    }
    pub fn message_upper_address(&self) -> Option<u32> {
        match self {
            &Self::_64BitAddress { message_address_hi, .. } | &Self::_64BitAddressWithPvm { message_address_hi, .. } => Some(message_address_hi),
            &Self::_32BitAddress { .. } | &Self::_32BitAddressWithPvm { .. } => None,
        }
    }
    pub fn message_data(&self) -> u16 {
        match self {
            &Self::_32BitAddress { message_data, .. } | &Self::_64BitAddress { message_data, .. } => message_data,
            &Self::_32BitAddressWithPvm { message_data, .. } | &Self::_64BitAddressWithPvm { message_data, .. } => message_data as u16,
        }
    }
    pub fn mask_bits(&self) -> Option<u32> {
        match self {
            &Self::_32BitAddressWithPvm { mask_bits, .. } | &Self::_64BitAddressWithPvm { mask_bits, .. } => Some(mask_bits),
            &Self::_32BitAddress { .. } | &Self::_64BitAddress { .. } => None,
        }
    }
    pub fn pending_bits(&self) -> Option<u32> {
        match self {
            &Self::_32BitAddressWithPvm { pending_bits, .. } | &Self::_64BitAddressWithPvm { pending_bits, .. } => Some(pending_bits),
            &Self::_32BitAddress { .. } | &Self::_64BitAddress { .. } => None,
        }
    }
    pub fn set_message_address(&mut self, message_address: u32) {
        assert_eq!(message_address & 0xFFFF_FFFC, message_address, "unaligned message address (this should already be validated)");
        match self {
            &mut Self::_32BitAddress { message_address: ref mut addr, .. } | &mut Self::_32BitAddressWithPvm { message_address: ref mut addr, .. } => *addr = message_address,
            &mut Self::_64BitAddress { message_address_lo: ref mut addr, .. } | &mut Self::_64BitAddressWithPvm { message_address_lo: ref mut addr, .. } => *addr = message_address,
        }
    }
    pub fn set_message_upper_address(&mut self, message_upper_address: u32) -> Option<()> {
        match self {
            &mut Self::_64BitAddress { ref mut message_address_hi, .. } | &mut Self::_64BitAddressWithPvm { ref mut message_address_hi, .. } => *message_address_hi = message_upper_address,
            &mut Self::_32BitAddress { .. } | &mut Self::_32BitAddressWithPvm { .. } => return None,
        }
        Some(())
    }
    pub fn set_message_data(&mut self, value: u16) {
        match self {
            &mut Self::_32BitAddress { ref mut message_data, .. } | &mut Self::_64BitAddress { ref mut message_data, .. } => *message_data = value,
            &mut Self::_32BitAddressWithPvm { ref mut message_data, .. } | &mut Self::_64BitAddressWithPvm { ref mut message_data, .. } => {
                *message_data &= 0xFFFF_0000;
                *message_data |= u32::from(value);
            }
        }
    }
    pub fn set_mask_bits(&mut self, mask_bits: u32) -> Option<()> {
        match self {
            &mut Self::_32BitAddressWithPvm { mask_bits: ref mut bits, .. } | &mut Self::_64BitAddressWithPvm { mask_bits: ref mut bits, .. } => *bits = mask_bits,
            &mut Self::_32BitAddress { .. } | &mut Self::_64BitAddress { .. } => return None,
        }
        Some(())
    }
    pub unsafe fn write_message_address<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(u16::from(offset) + 4, self.message_address())
    }
    pub unsafe fn write_message_upper_address<W: ConfigWriter>(&self, writer: &W, offset: u8) -> Option<()> {
        let value = self.message_upper_address()?;
        writer.write_u32(u16::from(offset + 8), value);
        Some(())
    }
    pub unsafe fn write_message_data<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        match self {
            &Self::_32BitAddress { message_data, .. } => writer.write_u32(u16::from(offset + 8), message_data.into()),
            &Self::_32BitAddressWithPvm { message_data, .. } => writer.write_u32(u16::from(offset + 8), message_data),
            &Self::_64BitAddress { message_data, .. } => writer.write_u32(u16::from(offset + 12), message_data.into()),
            &Self::_64BitAddressWithPvm { message_data, .. } => writer.write_u32(u16::from(offset + 12), message_data),
        }
    }
    pub unsafe fn write_mask_bits<W: ConfigWriter>(&self, writer: &W, offset: u8) -> Option<()> {
        match self {
            &Self::_32BitAddressWithPvm { mask_bits, .. } => writer.write_u32(u16::from(offset + 12), mask_bits),
            &Self::_64BitAddressWithPvm { mask_bits, .. } => writer.write_u32(u16::from(offset + 16), mask_bits),
            &Self::_32BitAddress { .. } | &Self::_64BitAddress { .. } => return None,
        }
        Some(())
    }
    pub unsafe fn write_all<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        self.write_message_control(writer, offset);
        self.write_message_address(writer, offset);
        self.write_message_upper_address(writer, offset);
        self.write_message_data(writer, offset);
        self.write_mask_bits(writer, offset);
    }
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
        self.c & Self::PBA_OFFSET_MASK
    }
    /// The Pending Bit Array BIR, which is used to map the offset to a memory location.
    pub const fn pba_bir(&self) -> u8 {
        (self.c & Self::PBA_BIR_MASK) as u8
    }

    pub fn set_pba_offset(&mut self, offset: u32) {
        assert_eq!(offset & Self::PBA_OFFSET_MASK, offset, "MSI-X Pending Bit Array offset has to be QWORD aligned");
        self.c &= !Self::PBA_OFFSET_MASK;
        self.c |= offset;
    }

    pub fn table_base_pointer(&self, bars: [PciBar; 6]) -> usize {
        if self.table_bir() > 5 {
            panic!("MSI-X Table BIR contained a reserved enum value: {}", self.table_bir());
        }
        let base = bars[usize::from(self.table_bir())];

        if let PciBar::Memory(ptr) = base {
            ptr as usize + self.table_offset() as usize
        } else {
            panic!("MSI-X Table BIR referenced a non-memory BAR: {:?}", base);
        }
    }
    pub fn table_pointer(&self, bars: [PciBar; 6], k: u16) -> usize {
        self.table_base_pointer(bars) + k as usize * 16
    }

    pub fn pba_base_pointer(&self, bars: [PciBar; 6]) -> usize {
        if self.pba_bir() > 5 {
            panic!("MSI-X PBA BIR contained a reserved enum value: {}", self.pba_bir());
        }
        let base = bars[usize::from(self.pba_bir())];

        if let PciBar::Memory(ptr) = base {
            ptr as usize + self.pba_offset() as usize
        } else {
            panic!("MSI-X PBA BIR referenced a non-memory BAR: {:?}", base);
        }
    }
    pub fn pba_pointer_dword(&self, bars: [PciBar; 6], k: u16) -> usize {
        self.pba_base_pointer(bars) + (k as usize / 32) * 4
    }
    pub const fn pba_bit_dword(&self, k: u16) -> u8 {
        (k % 32) as u8
    }

    pub fn pba_pointer_qword(&self, bars: [PciBar; 6], k: u16) -> usize {
        self.pba_base_pointer(bars) + (k as usize / 64) * 8
    }
    pub const fn pba_bit_qword(&self, k: u16) -> u8 {
        (k % 64) as u8
    }

    /// Write the first DWORD into configuration space (containing the partially modifiable Message
    /// Control field).
    pub unsafe fn write_a<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(u16::from(offset), self.a)
    }
    /// Write the second DWORD into configuration space (containing the modifiable table
    /// offset and the readonly table BIR).
    pub unsafe fn write_b<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(u16::from(offset + 4), self.a)
    }
    /// Write the third DWORD into configuration space (containing the modifiable pending bit array
    /// offset, and the readonly PBA BIR).
    pub unsafe fn write_c<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        writer.write_u32(u16::from(offset + 8), self.a)
    }
    /// Write this capability structure back to configuration space.
    pub unsafe fn write_all<W: ConfigWriter>(&self, writer: &W, offset: u8) {
        self.write_a(writer, offset);
        self.write_b(writer, offset);
        self.write_c(writer, offset);
    }
}

#[repr(packed)]
pub struct MsixTableEntry {
    pub addr_lo: Mmio<u32>,
    pub addr_hi: Mmio<u32>,
    pub msg_data: Mmio<u32>,
    pub vec_ctl: Mmio<u32>,
}

#[cfg(target_arch = "x86_64")]
pub mod x86_64 {
    #[repr(u8)]
    pub enum TriggerMode {
        Edge = 0,
        Level = 1,
    }

    #[repr(u8)]
    pub enum LevelTriggerMode {
        Deassert = 0,
        Assert = 1,
    }

    #[repr(u8)]
    pub enum DeliveryMode {
        Fixed = 0b000,
        LowestPriority = 0b001,
        Smi = 0b010,
        // 0b011 is reserved
        Nmi = 0b100,
        Init = 0b101,
        // 0b110 is reserved
        ExtInit = 0b111,
    }

    // TODO: should the reserved field be preserved?
    pub const fn message_address(destination_id: u8, rh: bool, dm: bool) -> u32 {
        0xFEE0_0000u32
            | ((destination_id as u32) << 12)
            | ((rh as u32) << 3)
            | ((dm as u32) << 2)
    }
    pub const fn message_data(trigger_mode: TriggerMode, level_trigger_mode: LevelTriggerMode, delivery_mode: DeliveryMode, vector: u8) -> u32 {
        ((trigger_mode as u32) << 15)
            | ((level_trigger_mode as u32) << 14)
            | ((delivery_mode as u32) << 8)
            | vector as u32
    }
    pub const fn message_data_level_triggered(level_trigger_mode: LevelTriggerMode, delivery_mode: DeliveryMode, vector: u8) -> u32 {
        message_data(TriggerMode::Level, level_trigger_mode, delivery_mode, vector)
    }
    pub const fn message_data_edge_triggered(delivery_mode: DeliveryMode, vector: u8) -> u32 {
        message_data(TriggerMode::Edge, LevelTriggerMode::Deassert, delivery_mode, vector)
    }
}

impl MsixTableEntry {
    pub fn addr_lo(&self) -> u32 {
        self.addr_lo.read()
    }
    pub fn addr_hi(&self) -> u32 {
        self.addr_hi.read()
    }
    pub fn set_addr_lo(&mut self, value: u32) {
        self.addr_lo.write(value);
    }
    pub fn set_addr_hi(&mut self, value: u32) {
        self.addr_hi.write(value);
    }
    pub fn msg_data(&self) -> u32 {
        self.msg_data.read()
    }
    pub fn vec_ctl(&self) -> u32 {
        self.vec_ctl.read()
    }
    pub fn set_msg_data(&mut self, value: u32) {
        self.msg_data.write(value);
    }
    pub fn addr(&self) -> u64 {
        u64::from(self.addr_lo()) | (u64::from(self.addr_hi()) << 32)
    }
    pub const VEC_CTL_MASK_BIT: u32 = 1;

    pub fn set_masked(&mut self, masked: bool) {
        self.vec_ctl.writef(Self::VEC_CTL_MASK_BIT, masked)
    }
    pub fn mask(&mut self) {
        self.set_masked(true);
    }
    pub fn unmask(&mut self) {
        self.set_masked(false);
    }
}

impl fmt::Debug for MsixTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("MsixTableEntry")
            .field("addr", &self.addr())
            .field("msg_data", &self.msg_data())
            .field("vec_ctl", &self.vec_ctl())
            .finish()
    }
}
