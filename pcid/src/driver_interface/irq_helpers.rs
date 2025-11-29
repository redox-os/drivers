//! IRQ helpers.
//!
//! This module allows easy handling of the `/scheme/irq` scheme, and allocating interrupt vectors
//! for use by INTx#, MSI, or MSI-X.

use std::convert::TryFrom;
use std::fs::{self, File};
use std::io::{self, prelude::*};
use std::num::NonZeroU8;

use crate::driver_interface::msi::{MsiAddrAndData, MsixTableEntry};

/// Read the local APIC ID of the bootstrap processor.
pub fn read_bsp_apic_id() -> io::Result<usize> {
    let mut buffer = [0u8; 8];

    let mut file = File::open("/scheme/irq/bsp")?;
    let bytes_read = file.read(&mut buffer)?;

    (if bytes_read == 8 {
        usize::try_from(u64::from_le_bytes(buffer))
    } else if bytes_read == 4 {
        usize::try_from(u32::from_le_bytes([
            buffer[0], buffer[1], buffer[2], buffer[3],
        ]))
    } else {
        panic!(
            "`/scheme/irq` scheme responded with {} bytes, expected {}",
            bytes_read,
            std::mem::size_of::<usize>()
        );
    })
    .or(Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "bad BSP int size",
    )))
}

// TODO: Perhaps read the MADT instead?
/// Obtains an interator over all of the visible CPU ids, for use in IRQ allocation and MSI
/// capability structs or MSI-X tables.
pub fn cpu_ids() -> io::Result<impl Iterator<Item = io::Result<usize>> + 'static> {
    Ok(
        fs::read_dir("/scheme/irq")?.filter_map(|entry| -> Option<io::Result<_>> {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    let file_name = path.file_name()?.to_str()?;
                    // the file name should be in the format `cpu-<CPU ID>`
                    if !file_name.starts_with("cpu-") {
                        return None;
                    }
                    u8::from_str_radix(&file_name[4..], 16)
                        .map(usize::from)
                        .map(Ok)
                        .ok()
                }
                Err(e) => Some(Err(e)),
            }
        }),
    )
}

/// Allocate multiple interrupt vectors, from the IDT of the specified processor, returning the
/// start vector and the IRQ handles.
///
/// The alignment is a requirement for the allocation range. For example, with an alignment of 8,
/// only ranges that begin with a multiple of eight are accepted. The IRQ handles returned will
/// always correspond to the subsequent IRQ numbers beginning the first value in the return tuple.
///
/// This function is not actually guaranteed to allocate all of the IRQs specified in `count`,
/// since another process might already have requested one vector in the range. The caller must
/// check that the returned vector have the same length as `count`. In the future this function may
/// perhaps lock the entire directory to prevent this from happening, or maybe find the smallest free
/// range with the minimum alignment, to allow other drivers to obtain their necessary IRQs.
///
/// Note that this count/alignment restriction is only mandatory for MSI; MSI-X allows for
/// individually allocated vectors that might be spread out, even on multiple CPUs. Thus, multiple
/// invocations with alignment 1 and count 1 are totally acceptable, although allocating in bulk
/// minimizes the initialization overhead.
pub fn allocate_aligned_interrupt_vectors(
    cpu_id: usize,
    alignment: NonZeroU8,
    count: u8,
) -> io::Result<Option<(u8, Vec<File>)>> {
    let cpu_id = u8::try_from(cpu_id).expect("usize cpu ids not implemented yet");
    if count == 0 {
        return Ok(None);
    }

    let available_irqs = fs::read_dir(format!("/scheme/irq/cpu-{:02x}", cpu_id))?;
    let mut available_irq_numbers = available_irqs.filter_map(|entry| -> Option<io::Result<_>> {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => return Some(Err(err)),
        };

        let path = entry.path();

        let file_name = match path.file_name() {
            Some(f) => f,
            None => return None,
        };

        let path_str = match file_name.to_str() {
            Some(s) => s,
            None => return None,
        };

        match path_str.parse::<u8>() {
            Ok(p) => Some(Ok(p)),
            Err(_) => None,
        }
    });

    // TODO: fcntl F_SETLK on `/scheme/irq/`?

    let mut handles = Vec::with_capacity(usize::from(count));

    let mut index = 0;
    let mut first = None;

    while let Some(number) = available_irq_numbers.next() {
        let number = number?;

        // Skip until a suitable alignment is found.
        if number % u8::from(alignment) != 0 {
            continue;
        }
        let first = *first.get_or_insert(number);
        let irq_number = first + index;

        // From the point where the range is aligned, we can start to advance until `count` IRQs
        // have been allocated.
        if index >= count {
            break;
        }

        // if found, reserve the irq
        let irq_handle =
            match File::create(format!("/scheme/irq/cpu-{:02x}/{}", cpu_id, irq_number)) {
                Ok(handle) => handle,

                // return early if the entire range couldn't be allocated
                Err(err) if err.kind() == io::ErrorKind::NotFound => break,

                Err(err) => return Err(err),
            };
        handles.push(irq_handle);
        index += 1;
    }
    if handles.is_empty() {
        return Ok(None);
    }
    let first = match first {
        Some(f) => f,
        None => return Ok(None),
    };

    Ok(Some((first + 32, handles)))
}

/// Allocate at most `count` interrupt vectors, which can start at any offset. Unless MSI is used
/// and an entire aligned range of vectors is needed, this function should be used.
pub fn allocate_interrupt_vectors(cpu_id: usize, count: u8) -> io::Result<Option<(u8, Vec<File>)>> {
    allocate_aligned_interrupt_vectors(cpu_id, NonZeroU8::new(1).unwrap(), count)
}

/// Allocate a single interrupt vector, returning both the vector number (starting from 32 up to
/// 254), and its IRQ handle which is then reserved. Returns Ok(None) if allocation fails due to
/// no available IRQs.
pub fn allocate_single_interrupt_vector(cpu_id: usize) -> io::Result<Option<(u8, File)>> {
    let (base, mut files) = match allocate_interrupt_vectors(cpu_id, 1) {
        Ok(Some((base, files))) => (base, files),
        Ok(None) => return Ok(None),
        Err(err) => return Err(err),
    };
    assert_eq!(files.len(), 1);
    Ok(Some((base, files.pop().unwrap())))
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn allocate_single_interrupt_vector_for_msi(cpu_id: usize) -> (MsiAddrAndData, File) {
    use crate::driver_interface::msi::x86 as x86_msix;

    // FIXME for cpu_id >255 we need to use the IOMMU to use IRQ remapping
    let lapic_id = u8::try_from(cpu_id).expect("CPU id couldn't fit inside u8");
    let rh = false;
    let dm = false;
    let addr = x86_msix::message_address(lapic_id, rh, dm);

    let (vector, interrupt_handle) = allocate_single_interrupt_vector(cpu_id)
        .expect("failed to allocate interrupt vector")
        .expect("no interrupt vectors left");
    let msg_data = x86_msix::message_data_edge_triggered(x86_msix::DeliveryMode::Fixed, vector);

    (
        MsiAddrAndData {
            addr,
            data: msg_data,
        },
        interrupt_handle,
    )
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn allocate_first_msi_interrupt_on_bsp(
    pcid_handle: &mut crate::driver_interface::PciFunctionHandle,
) -> File {
    use crate::driver_interface::{MsiSetFeatureInfo, PciFeature, SetFeatureInfo};

    // TODO: Allow allocation of up to 32 vectors.

    let destination_id = read_bsp_apic_id().expect("failed to read BSP apic id");
    let (msg_addr_and_data, interrupt_handle) =
        allocate_single_interrupt_vector_for_msi(destination_id);

    let set_feature_info = MsiSetFeatureInfo {
        multi_message_enable: Some(0),
        message_address_and_data: Some(msg_addr_and_data),
        mask_bits: None,
    };
    pcid_handle.set_feature_info(SetFeatureInfo::Msi(set_feature_info));

    pcid_handle.enable_feature(PciFeature::Msi);
    log::debug!("Enabled MSI");

    interrupt_handle
}

pub struct InterruptVector {
    irq_handle: File,
    vector: u16,
    kind: InterruptVectorKind,
}

enum InterruptVectorKind {
    Legacy,
    Msi,
    MsiX { table_entry: *mut MsixTableEntry },
}

impl InterruptVector {
    pub fn irq_handle(&self) -> &File {
        &self.irq_handle
    }

    pub fn vector(&self) -> u16 {
        self.vector
    }

    pub fn set_masked_if_fast(&mut self, masked: bool) -> bool {
        match self.kind {
            InterruptVectorKind::Legacy | InterruptVectorKind::Msi => false,
            InterruptVectorKind::MsiX { table_entry } => {
                unsafe { (*table_entry).set_masked(masked) };
                true
            }
        }
    }
}

/// Get the most optimal supported interrupt mechanism: either (in the order of preference):
/// MSI-X, MSI, and INTx# pin. Returns both runtime interrupt structures (MSI/MSI-X capability
/// structures), and the handles to the interrupts.
// FIXME allow allocating multiple interrupt vectors
// FIXME move MSI-X IRQ allocation to pcid
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn pci_allocate_interrupt_vector(
    pcid_handle: &mut crate::driver_interface::PciFunctionHandle,
    driver: &str,
) -> InterruptVector {
    let features = pcid_handle.fetch_all_features();

    let has_msi = features.iter().any(|feature| feature.is_msi());
    let has_msix = features.iter().any(|feature| feature.is_msix());

    if has_msix {
        let msix_info = match pcid_handle.feature_info(super::PciFeature::MsiX) {
            super::PciFeatureInfo::MsiX(msix) => msix,
            _ => unreachable!(),
        };
        let mut info = unsafe { msix_info.map_and_mask_all(pcid_handle) };

        pcid_handle.enable_feature(crate::driver_interface::PciFeature::MsiX);

        let entry = info.table_entry_pointer(0);

        let bsp_cpu_id = read_bsp_apic_id()
            .unwrap_or_else(|err| panic!("{driver}: failed to read BSP APIC ID: {err}"));
        let (msg_addr_and_data, irq_handle) = allocate_single_interrupt_vector_for_msi(bsp_cpu_id);
        entry.write_addr_and_data(msg_addr_and_data);
        entry.unmask();

        InterruptVector {
            irq_handle,
            vector: 0,
            kind: InterruptVectorKind::MsiX { table_entry: entry },
        }
    } else if has_msi {
        InterruptVector {
            irq_handle: allocate_first_msi_interrupt_on_bsp(pcid_handle),
            vector: 0,
            kind: InterruptVectorKind::Msi,
        }
    } else if let Some(irq) = pcid_handle.config().func.legacy_interrupt_line {
        // INTx# pin based interrupts.
        InterruptVector {
            irq_handle: irq.irq_handle(driver),
            vector: 0,
            kind: InterruptVectorKind::Legacy,
        }
    } else {
        panic!("{driver}: no interrupts supported at all")
    }
}

// FIXME support MSI on non-x86 systems
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub fn pci_allocate_interrupt_vector(
    pcid_handle: &mut crate::driver_interface::PciFunctionHandle,
    driver: &str,
) -> InterruptVector {
    if let Some(irq) = pcid_handle.config().func.legacy_interrupt_line {
        // INTx# pin based interrupts.
        InterruptVector {
            irq_handle: irq.irq_handle(driver),
            vector: 0,
            kind: InterruptVectorKind::Legacy,
        }
    } else {
        panic!("{driver}: no interrupts supported at all")
    }
}
