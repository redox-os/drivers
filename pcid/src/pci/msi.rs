use std::fmt;

use super::bar::PciBar;
pub use super::cap::{MsiCapability, MsixCapability};
use super::func::PciFunc;

use serde::{Deserialize, Serialize};
use syscall::{Io, Mmio};

/// The address and data to use for MSI and MSI-X.
///
/// For MSI using this only works when you need a single interrupt vector.
/// For MSI-X you can have a single [MsiEntry] for each interrupt vector.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MsiAddrAndData {
    pub(crate) addr: u64,
    pub(crate) data: u32,
}

impl MsiAddrAndData {
    pub fn new(addr: u64, data: u32) -> Self {
        MsiAddrAndData { addr, data }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MsiInfo {
    pub log2_multiple_message_capable: u8,
    pub is_64bit: bool,
    pub has_per_vector_masking: bool,
}

impl MsiCapability {
    const MC_PVT_CAPABLE_BIT: u16 = 1 << 8;
    const MC_64_BIT_ADDR_BIT: u16 = 1 << 7;

    const MC_MULTI_MESSAGE_MASK: u16 = 0x000E;
    const MC_MULTI_MESSAGE_SHIFT: u8 = 1;

    const MC_MULTI_MESSAGE_ENABLE_MASK: u16 = 0x0070;
    const MC_MULTI_MESSAGE_ENABLE_SHIFT: u8 = 4;

    const MC_MSI_ENABLED_BIT: u16 = 1;

    pub(crate) unsafe fn parse(func: &PciFunc, offset: u8) -> Self {
        let dword = func.pci.read(func.addr, u16::from(offset));

        let message_control = (dword >> 16) as u16;

        if message_control & Self::MC_PVT_CAPABLE_BIT != 0 {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddressWithPvm {
                    cap_offset: offset,
                    message_control: dword,
                    message_address_lo: func.pci.read(func.addr, u16::from(offset + 4)),
                    message_address_hi: func.pci.read(func.addr, u16::from(offset + 8)),
                    message_data: func.pci.read(func.addr, u16::from(offset + 12)),
                    mask_bits: func.pci.read(func.addr, u16::from(offset + 16)),
                    pending_bits: func.pci.read(func.addr, u16::from(offset + 20)),
                }
            } else {
                Self::_32BitAddressWithPvm {
                    cap_offset: offset,
                    message_control: dword,
                    message_address: func.pci.read(func.addr, u16::from(offset + 4)),
                    message_data: func.pci.read(func.addr, u16::from(offset + 8)),
                    mask_bits: func.pci.read(func.addr, u16::from(offset + 12)),
                    pending_bits: func.pci.read(func.addr, u16::from(offset + 16)),
                }
            }
        } else {
            if message_control & Self::MC_64_BIT_ADDR_BIT != 0 {
                Self::_64BitAddress {
                    cap_offset: offset,
                    message_control: dword,
                    message_address_lo: func.pci.read(func.addr, u16::from(offset + 4)),
                    message_address_hi: func.pci.read(func.addr, u16::from(offset + 8)),
                    message_data: func.pci.read(func.addr, u16::from(offset + 12)) as u16,
                }
            } else {
                Self::_32BitAddress {
                    cap_offset: offset,
                    message_control: dword,
                    message_address: func.pci.read(func.addr, u16::from(offset + 4)),
                    message_data: func.pci.read(func.addr, u16::from(offset + 8)) as u16,
                }
            }
        }
    }
    fn cap_offset(&self) -> u16 {
        match *self {
            MsiCapability::_32BitAddress { cap_offset, .. }
            | MsiCapability::_64BitAddress { cap_offset, .. }
            | MsiCapability::_32BitAddressWithPvm { cap_offset, .. }
            | MsiCapability::_64BitAddressWithPvm { cap_offset, .. } => u16::from(cap_offset),
        }
    }

    fn message_control_raw(&self) -> u32 {
        match self {
            Self::_32BitAddress { message_control, .. } | Self::_64BitAddress { message_control, .. } | Self::_32BitAddressWithPvm { message_control, .. } | Self::_64BitAddressWithPvm { message_control, .. } => *message_control,
        }
    }
    fn message_control(&self) -> u16 {
        (self.message_control_raw() >> 16) as u16
    }
    pub(crate) fn set_message_control(&mut self, value: u16) {
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
    pub(crate) unsafe fn write_message_control(&self, func: &PciFunc) {
        func.pci.write(func.addr, self.cap_offset(), self.message_control_raw());
    }
    pub(crate) fn is_pvt_capable(&self) -> bool {
        self.message_control() & Self::MC_PVT_CAPABLE_BIT != 0
    }
    pub(crate) fn has_64_bit_addr(&self) -> bool {
        self.message_control() & Self::MC_64_BIT_ADDR_BIT != 0
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        let mut new_message_control = self.message_control() & (!Self::MC_MSI_ENABLED_BIT);
        new_message_control |= u16::from(enabled);
        self.set_message_control(new_message_control);
    }
    pub(crate) fn multi_message_capable(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_MASK) >> Self::MC_MULTI_MESSAGE_SHIFT) as u8
    }
    pub(crate) fn multi_message_enable(&self) -> u8 {
        ((self.message_control() & Self::MC_MULTI_MESSAGE_ENABLE_MASK) >> Self::MC_MULTI_MESSAGE_ENABLE_SHIFT) as u8
    }
    pub(crate) fn set_multi_message_enable(&mut self, log_mme: u8) {
        let mut new_message_control = self.message_control() & (!Self::MC_MULTI_MESSAGE_ENABLE_MASK);
        new_message_control |= u16::from(log_mme) << Self::MC_MULTI_MESSAGE_ENABLE_SHIFT;
        self.set_message_control(new_message_control);
    }

    fn message_address(&self) -> u32 {
        match self {
            &Self::_32BitAddress { message_address, .. } | &Self::_32BitAddressWithPvm { message_address, .. } => message_address,
            &Self::_64BitAddress { message_address_lo, .. } | &Self::_64BitAddressWithPvm { message_address_lo, .. } => message_address_lo,
        }
    }
    fn message_upper_address(&self) -> Option<u32> {
        match self {
            &Self::_64BitAddress { message_address_hi, .. } | &Self::_64BitAddressWithPvm { message_address_hi, .. } => Some(message_address_hi),
            &Self::_32BitAddress { .. } | &Self::_32BitAddressWithPvm { .. } => None,
        }
    }
    pub(crate) fn set_message_address(&mut self, message_address: u32) {
        assert_eq!(message_address & 0xFFFF_FFFC, message_address, "unaligned message address (this should already be validated)");
        match self {
            &mut Self::_32BitAddress { message_address: ref mut addr, .. } | &mut Self::_32BitAddressWithPvm { message_address: ref mut addr, .. } => *addr = message_address,
            &mut Self::_64BitAddress { message_address_lo: ref mut addr, .. } | &mut Self::_64BitAddressWithPvm { message_address_lo: ref mut addr, .. } => *addr = message_address,
        }
    }
    pub(crate) fn set_message_upper_address(&mut self, message_upper_address: u32) -> Option<()> {
        match self {
            &mut Self::_64BitAddress { ref mut message_address_hi, .. } | &mut Self::_64BitAddressWithPvm { ref mut message_address_hi, .. } => *message_address_hi = message_upper_address,
            &mut Self::_32BitAddress { .. } | &mut Self::_32BitAddressWithPvm { .. } => return None,
        }
        Some(())
    }
    pub(crate) fn set_message_data(&mut self, value: u16) {
        match self {
            &mut Self::_32BitAddress { ref mut message_data, .. } | &mut Self::_64BitAddress { ref mut message_data, .. } => *message_data = value,
            &mut Self::_32BitAddressWithPvm { ref mut message_data, .. } | &mut Self::_64BitAddressWithPvm { ref mut message_data, .. } => {
                *message_data &= 0xFFFF_0000;
                *message_data |= u32::from(value);
            }
        }
    }
    pub(crate) fn set_mask_bits(&mut self, mask_bits: u32) -> Option<()> {
        match self {
            &mut Self::_32BitAddressWithPvm { mask_bits: ref mut bits, .. } | &mut Self::_64BitAddressWithPvm { mask_bits: ref mut bits, .. } => *bits = mask_bits,
            &mut Self::_32BitAddress { .. } | &mut Self::_64BitAddress { .. } => return None,
        }
        Some(())
    }
    unsafe fn write_message_address(&self, func: &PciFunc) {
        func.pci.write(func.addr, self.cap_offset() + 4, self.message_address())
    }
    unsafe fn write_message_upper_address(&self, func: &PciFunc) -> Option<()> {
        let value = self.message_upper_address()?;
        func.pci.write(func.addr, self.cap_offset() + 8, value);
        Some(())
    }
    unsafe fn write_message_data(&self, func: &PciFunc) {
        match self {
            &Self::_32BitAddress { cap_offset, message_data, .. } => func.pci.write(func.addr, u16::from(cap_offset + 8), message_data.into()),
            &Self::_32BitAddressWithPvm { cap_offset, message_data, .. } => func.pci.write(func.addr, u16::from(cap_offset + 8), message_data),
            &Self::_64BitAddress { cap_offset, message_data, .. } => func.pci.write(func.addr, u16::from(cap_offset + 12), message_data.into()),
            &Self::_64BitAddressWithPvm { cap_offset, message_data, .. } => func.pci.write(func.addr, u16::from(cap_offset + 12), message_data),
        }
    }
    unsafe fn write_mask_bits(&self, func: &PciFunc) -> Option<()> {
        match self {
            &Self::_32BitAddressWithPvm { cap_offset, mask_bits, .. } => func.pci.write(func.addr, u16::from(cap_offset + 12), mask_bits),
            &Self::_64BitAddressWithPvm { cap_offset, mask_bits, .. } => func.pci.write(func.addr, u16::from(cap_offset + 16), mask_bits),
            &Self::_32BitAddress { .. } | &Self::_64BitAddress { .. } => return None,
        }
        Some(())
    }
    pub(crate) unsafe fn write_all(&self, func: &PciFunc) {
        self.write_message_control(func);
        self.write_message_address(func);
        self.write_message_upper_address(func);
        self.write_message_data(func);
        self.write_mask_bits(func);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MsixInfo {
    pub table_bar: u8,
    pub table_offset: u32,
    pub table_size: u16,
    pub pba_bar: u8,
    pub pba_offset: u32,
}

impl MsixInfo {
    pub fn validate(&self, bars: [PciBar; 6]) {
        if self.table_bar > 5 {
            panic!("MSI-X Table BIR contained a reserved enum value: {}", self.table_bar);
        }
        if self.pba_bar > 5 {
            panic!("MSI-X PBA BIR contained a reserved enum value: {}", self.pba_bar);
        }

        let table_size = self.table_size;
        let table_offset = self.table_offset as usize;
        let table_min_length = table_size * 16;

        let pba_offset = self.pba_offset as usize;
        let pba_min_length = table_size.div_ceil(8);

        let (_, table_bar_size) = bars[self.table_bar as usize].expect_mem();
        let (_, pba_bar_size) = bars[self.pba_bar as usize].expect_mem();

        // Ensure that the table and PBA are within the BAR.

        if !(0..table_bar_size as u64).contains(&(table_offset as u64 + table_min_length as u64)) {
            panic!(
                "Table {:#x}:{:#x} outside of BAR with length {:#x}",
                table_offset,
                table_offset + table_min_length as usize,
                table_bar_size
            );
        }

        if !(0..pba_bar_size as u64).contains(&(pba_offset as u64 + pba_min_length as u64)) {
            panic!(
                "PBA {:#x}:{:#x} outside of BAR with length {:#x}",
                pba_offset,
                pba_offset + pba_min_length as usize,
                pba_bar_size
            );
        }
    }
}

impl MsixCapability {
    const MC_MSIX_ENABLED_BIT: u16 = 1 << 15;
    const MC_MSIX_ENABLED_SHIFT: u8 = 15;
    const MC_FUNCTION_MASK_BIT: u16 = 1 << 14;
    const MC_FUNCTION_MASK_SHIFT: u8 = 14;
    const MC_TABLE_SIZE_MASK: u16 = 0x03FF;

    /// The Message Control field, containing the enabled and function mask bits, as well as the
    /// table size.
    const fn message_control(&self) -> u16 {
        (self.a >> 16) as u16
    }

    pub(crate) fn set_message_control(&mut self, message_control: u16) {
        self.a &= 0x0000_FFFF;
        self.a |= u32::from(message_control) << 16;
    }
    /// Returns the MSI-X table size.
    pub(crate) const fn table_size(&self) -> u16 {
        (self.message_control() & Self::MC_TABLE_SIZE_MASK) + 1
    }
    pub(crate) fn set_msix_enabled(&mut self, enabled: bool) {
        let mut new_message_control = self.message_control();
        new_message_control &= !(Self::MC_MSIX_ENABLED_BIT);
        new_message_control |= u16::from(enabled) << Self::MC_MSIX_ENABLED_SHIFT;
        self.set_message_control(new_message_control);
    }

    pub(crate) fn set_function_mask(&mut self, function_mask: bool) {
        let mut new_message_control = self.message_control();
        new_message_control &= !(Self::MC_FUNCTION_MASK_BIT);
        new_message_control |= u16::from(function_mask) << Self::MC_FUNCTION_MASK_SHIFT;
        self.set_message_control(new_message_control);
    }
    const TABLE_OFFSET_MASK: u32 = 0xFFFF_FFF8;
    const TABLE_BIR_MASK: u32 = 0x0000_0007;

    /// The table offset is guaranteed to be QWORD aligned (8 bytes).
    pub(crate) const fn table_offset(&self) -> u32 {
        self.b & Self::TABLE_OFFSET_MASK
    }
    /// The table BIR, which is used to map the offset to a memory location.
    pub(crate) const fn table_bir(&self) -> u8 {
        (self.b & Self::TABLE_BIR_MASK) as u8
    }

    const PBA_OFFSET_MASK: u32 = 0xFFFF_FFF8;
    const PBA_BIR_MASK: u32 = 0x0000_0007;

    /// The Pending Bit Array offset is guaranteed to be QWORD aligned (8 bytes).
    pub(crate) const fn pba_offset(&self) -> u32 {
        self.c & Self::PBA_OFFSET_MASK
    }
    /// The Pending Bit Array BIR, which is used to map the offset to a memory location.
    pub(crate) const fn pba_bir(&self) -> u8 {
        (self.c & Self::PBA_BIR_MASK) as u8
    }


    /// Write the first DWORD into configuration space (containing the partially modifiable Message
    /// Control field).
    pub(crate) unsafe fn write_a(&self, func: &PciFunc) {
        func.pci.write(func.addr, u16::from(self.cap_offset), self.a)
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
    pub const fn message_address(destination_id: u8, redirect_hint: bool, dest_mode_logical: bool) -> u64 {
        0x0000_0000_FEE0_0000u64
            | ((destination_id as u64) << 12)
            | ((redirect_hint as u64) << 3)
            | ((dest_mode_logical as u64) << 2)
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

    pub fn write_addr_and_data(&mut self, entry: MsiAddrAndData) {
        self.set_addr_lo(entry.addr as u32);
        self.set_addr_hi((entry.addr >> 32) as u32);
        self.set_msg_data(entry.data);
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
