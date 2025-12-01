use common::io::{Io, MmioPtr};
use syscall::error::Result;

use super::{DeviceKind, MmioRegion};

// IHD-OS-TGL-Vol 2c-12.21 DDI_AUX_CTL
pub const DDI_AUX_CTL_BUSY: u32 = 1 << 31;
pub const DDI_AUX_CTL_DONE: u32 = 1 << 30;
pub const DDI_AUX_CTL_TIMEOUT_ERROR: u32 = 1 << 28;
pub const DDI_AUX_CTL_TIMEOUT_SHIFT: u32 = 26;
pub const DDI_AUX_CTL_TIMEOUT_MASK: u32 = 0b11 << DDI_AUX_CTL_TIMEOUT_SHIFT;
pub const DDI_AUX_CTL_TIMEOUT_4000US: u32 = 0b11 << DDI_AUX_CTL_TIMEOUT_SHIFT;
pub const DDI_AUX_CTL_RECEIVE_ERROR: u32 = 1 << 25;
pub const DDI_AUX_CTL_SIZE_SHIFT: u32 = 20;
pub const DDI_AUX_CTL_SIZE_MASK: u32 = 0b11111 << 20;
pub const DDI_AUX_CTL_IO_SELECT: u32 = 1 << 11;

// IHD-OS-TGL-Vol 2c-12.21 DDI_BUF_CTL
pub const DDI_BUF_CTL_ENABLE: u32 = 1 << 31;
pub const DDI_BUF_CTL_IDLE: u32 = 1 << 7;

pub struct Ddi {
    pub name: &'static str,
    pub index: usize,
    pub aux_ctl: MmioPtr<u32>,
    pub aux_datas: [MmioPtr<u32>; 5],
    pub buf_ctl: MmioPtr<u32>,
}

//TODO: verify offsets and count using DeviceKind?
impl Ddi {
    pub fn dpclka_cfgcr0_clock_off(&self) -> Option<u32> {
        match self.index {
            // DDI
            0 => Some(1 << 10),
            1 => Some(1 << 11),
            2 => Some(1 << 24),
            // Type C
            3 => Some(1 << 12),
            4 => Some(1 << 13),
            5 => Some(1 << 14),
            6 => Some(1 << 21),
            7 => Some(1 << 22),
            8 => Some(1 << 23),
            _ => None
        }
    }

    pub fn dpclka_cfgcr0_clock_shift(&self) -> Option<u32> {
        match self.index {
            0 => Some(0),
            1 => Some(2),
            2 => Some(4),
            _ => None
        }
    }

    pub fn gmbus_pin_pair(&self) -> Option<u8> {
        match self.index {
            // DDI pins
            0 => Some(1),
            1 => Some(2),
            2 => Some(3),
            // Type C pins
            3 => Some(9),
            4 => Some(10),
            5 => Some(11),
            6 => Some(12),
            7 => Some(13),
            8 => Some(14),
            _ => None
        }
    }

    pub fn port_cl_dw10(&self) -> Option<usize> {
        match self.index {
            0 => Some(0x162028),
            1 => Some(0x6C028),
            2 => Some(0x160028),
            _ => None,
        }
    }

    pub fn port_comp_dw0(&self) -> Option<usize> {
        match self.index {
            0 => Some(0x162100),
            1 => Some(0x6C100),
            2 => Some(0x160100),
            _ => None,
        }
    }

    pub fn pwr_well_ctl_aux_state(&self) -> u32 {
        1 << (self.index * 2)
    }

    pub fn pwr_well_ctl_aux_request(&self) -> u32 {
        2 << (self.index * 2)
    }

    pub fn pwr_well_ctl_ddi_state(&self) -> u32 {
        1 << (self.index * 2)
    }

    pub fn pwr_well_ctl_ddi_request(&self) -> u32 {
        2 << (self.index * 2)
    }

    pub fn transcoder_index(&self) -> u32 {
        (self.index + 1) as u32
    }

    pub fn tigerlake(gttmm: &MmioRegion) -> Result<Vec<Self>> {
        let mut ddis = Vec::new();
        for (i, name) in [
            "A",
            "B",
            "C",
            "USBC1",
            "USBC2",
            "USBC3",
            "USBC4",
            "USBC5",
            "USBC6",
        ].iter().enumerate() {
            ddis.push(Self {
                name,
                index: i,
                // IHD-OS-TGL-Vol 2c-12.21 DDI_AUX_CTL
                aux_ctl: unsafe { gttmm.mmio(0x64010 + i * 0x100)? },
                // IHD-OS-TGL-Vol 2c-12.21 DDI_AUX_DATA
                aux_datas: [
                    unsafe { gttmm.mmio(0x64014 + i * 0x100)? },
                    unsafe { gttmm.mmio(0x64018 + i * 0x100)? },
                    unsafe { gttmm.mmio(0x6401C + i * 0x100)? },
                    unsafe { gttmm.mmio(0x64020 + i * 0x100)? },
                    unsafe { gttmm.mmio(0x64024 + i * 0x100)? },
                ],
                // IHD-OS-TGL-Vol 2c-12.21 DDI_BUF_CTL
                buf_ctl: unsafe { gttmm.mmio(0x64000 + i * 0x100)? }
            })
        }
        Ok(ddis)
    }
}