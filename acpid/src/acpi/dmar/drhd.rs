use std::ops::{Deref, DerefMut};

use common::io::Mmio;

// TODO: Only wrap with Mmio where there are hardware-registers. (Some of these structs seem to be
// ring buffer entries, which are not to be treated the same way).

pub struct DrhdPage {
    virt: *mut Drhd,
}
impl DrhdPage {
    pub fn map(base_phys: usize) -> syscall::Result<Self> {
        assert_eq!(
            base_phys % crate::acpi::PAGE_SIZE,
            0,
            "DRHD registers must be page-aligned"
        );

        // TODO: Uncachable? Can reads have side-effects?
        let virt = unsafe {
            common::physmap(
                base_phys,
                crate::acpi::PAGE_SIZE,
                common::Prot::RO,
                common::MemoryType::default(),
            )?
        } as *mut Drhd;

        Ok(Self { virt })
    }
}
impl Deref for DrhdPage {
    type Target = Drhd;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.virt }
    }
}
impl DerefMut for DrhdPage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.virt }
    }
}
impl Drop for DrhdPage {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.virt.cast(), crate::acpi::PAGE_SIZE);
        }
    }
}

#[repr(C, packed)]
pub struct DrhdFault {
    pub sts: Mmio<u32>,
    pub ctrl: Mmio<u32>,
    pub data: Mmio<u32>,
    pub addr: [Mmio<u32>; 2],
    _rsv: [Mmio<u64>; 2],
    pub log: Mmio<u64>,
}

#[repr(C, packed)]
pub struct DrhdProtectedMemory {
    pub en: Mmio<u32>,
    pub low_base: Mmio<u32>,
    pub low_limit: Mmio<u32>,
    pub high_base: Mmio<u64>,
    pub high_limit: Mmio<u64>,
}

#[repr(C, packed)]
pub struct DrhdInvalidation {
    pub queue_head: Mmio<u64>,
    pub queue_tail: Mmio<u64>,
    pub queue_addr: Mmio<u64>,
    _rsv: Mmio<u32>,
    pub cmpl_sts: Mmio<u32>,
    pub cmpl_ctrl: Mmio<u32>,
    pub cmpl_data: Mmio<u32>,
    pub cmpl_addr: [Mmio<u32>; 2],
}

#[repr(C, packed)]
pub struct DrhdPageRequest {
    pub queue_head: Mmio<u64>,
    pub queue_tail: Mmio<u64>,
    pub queue_addr: Mmio<u64>,
    _rsv: Mmio<u32>,
    pub sts: Mmio<u32>,
    pub ctrl: Mmio<u32>,
    pub data: Mmio<u32>,
    pub addr: [Mmio<u32>; 2],
}

#[repr(C, packed)]
pub struct DrhdMtrrVariable {
    pub base: Mmio<u64>,
    pub mask: Mmio<u64>,
}

#[repr(C, packed)]
pub struct DrhdMtrr {
    pub cap: Mmio<u64>,
    pub def_type: Mmio<u64>,
    pub fixed: [Mmio<u64>; 11],
    pub variable: [DrhdMtrrVariable; 10],
}

#[repr(C, packed)]
pub struct Drhd {
    pub version: Mmio<u32>,
    _rsv: Mmio<u32>,
    pub cap: Mmio<u64>,
    pub ext_cap: Mmio<u64>,
    pub gl_cmd: Mmio<u32>,
    pub gl_sts: Mmio<u32>,
    pub root_table: Mmio<u64>,
    pub ctx_cmd: Mmio<u64>,
    _rsv1: Mmio<u32>,
    pub fault: DrhdFault,
    _rsv2: Mmio<u32>,
    pub pm: DrhdProtectedMemory,
    pub invl: DrhdInvalidation,
    _rsv3: Mmio<u64>,
    pub intr_table: Mmio<u64>,
    pub page_req: DrhdPageRequest,
    pub mtrr: DrhdMtrr,
}
