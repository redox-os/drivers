use std::fmt;
use std::ptr::NonNull;

use crate::driver_interface::PciBar;
use crate::PciFunctionHandle;

use common::io::{Io, Mmio};
use serde::{Deserialize, Serialize};

/// The address and data to use for MSI and MSI-X.
///
/// For MSI using this only works when you need a single interrupt vector.
/// For MSI-X you can have a single [MsiEntry] for each interrupt vector.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MsiAddrAndData {
    pub addr: u64,
    pub data: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MsiInfo {
    pub log2_multiple_message_capable: u8,
    pub is_64bit: bool,
    pub has_per_vector_masking: bool,
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
    pub unsafe fn map_and_mask_all(self, pcid_handle: &mut PciFunctionHandle) -> MappedMsixRegs {
        self.validate(pcid_handle.config().func.bars);

        let virt_table_base = unsafe {
            pcid_handle
                .map_bar(self.table_bar)
                .ptr
                .as_ptr()
                .byte_add(self.table_offset as usize)
        };

        let mut info = MappedMsixRegs {
            virt_table_base: NonNull::new(virt_table_base.cast::<MsixTableEntry>()).unwrap(),
            info: self,
        };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for i in 0..info.info.table_size {
            info.table_entry_pointer(i.into()).mask();
        }

        info
    }

    fn validate(&self, bars: [PciBar; 6]) {
        if self.table_bar > 5 {
            panic!(
                "MSI-X Table BIR contained a reserved enum value: {}",
                self.table_bar
            );
        }
        if self.pba_bar > 5 {
            panic!(
                "MSI-X PBA BIR contained a reserved enum value: {}",
                self.pba_bar
            );
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

pub struct MappedMsixRegs {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub info: MsixInfo,
}
impl MappedMsixRegs {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().add(k)
    }

    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.info.table_size as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

#[repr(C, packed)]
pub struct MsixTableEntry {
    pub addr_lo: Mmio<u32>,
    pub addr_hi: Mmio<u32>,
    pub msg_data: Mmio<u32>,
    pub vec_ctl: Mmio<u32>,
}

const _: () = {
    assert!(size_of::<MsixTableEntry>() == 16);
};

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub mod x86 {
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
    pub const fn message_address(
        destination_id: u8,
        redirect_hint: bool,
        dest_mode_logical: bool,
    ) -> u64 {
        0x0000_0000_FEE0_0000u64
            | ((destination_id as u64) << 12)
            | ((redirect_hint as u64) << 3)
            | ((dest_mode_logical as u64) << 2)
    }
    pub const fn message_data(
        trigger_mode: TriggerMode,
        level_trigger_mode: LevelTriggerMode,
        delivery_mode: DeliveryMode,
        vector: u8,
    ) -> u32 {
        ((trigger_mode as u32) << 15)
            | ((level_trigger_mode as u32) << 14)
            | ((delivery_mode as u32) << 8)
            | vector as u32
    }
    pub const fn message_data_level_triggered(
        level_trigger_mode: LevelTriggerMode,
        delivery_mode: DeliveryMode,
        vector: u8,
    ) -> u32 {
        message_data(
            TriggerMode::Level,
            level_trigger_mode,
            delivery_mode,
            vector,
        )
    }
    pub const fn message_data_edge_triggered(delivery_mode: DeliveryMode, vector: u8) -> u32 {
        message_data(
            TriggerMode::Edge,
            LevelTriggerMode::Deassert,
            delivery_mode,
            vector,
        )
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
