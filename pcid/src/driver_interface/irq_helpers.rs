//! IRQ helpers.
//!
//! This module allows easy handling of the `irq:` scheme, and allocating interrupt vectors for use
//! by INTx#, MSI, or MSI-X.

use std::fs::{self, File};
use std::io::{self, prelude::*};
use std::num::NonZeroU8;
use std::ops;

/// Read the local APIC ID of the bootstrap processor.
pub fn read_bsp_apic_id() -> io::Result<u32> {
    let mut buffer = [0u8; 8];

    let mut file = File::open("irq:bsp")?;
    let bytes_read = file.read(&mut buffer)?;

    Ok(if bytes_read == 8 {
        u64::from_le_bytes(buffer) as u32
    } else if bytes_read == 4 {
        u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]])
    } else {
        panic!("`irq:` scheme responded with {} bytes, expected {}", bytes_read, std::mem::size_of::<usize>());
    })
}

/// Allocate multiple interrupt vectors, from the IDT of the bootstrap processor, returning the
/// start vector and the IRQ handles.
///
/// The alignment is a requirement for the allocation range. For example, with an alignment of 8,
/// only ranges that begin with a multiple of eight are accepted. The IRQ handles returned will
/// always correspond to the subsequent IRQ numbers beginning the first value in the return tuple.
///
/// This function is not actually guaranteed to allocate all of the IRQs specified in `count`,
/// since another process might already have requested that vector. The caller must check that
/// the returned vector have the same length as `count`. In the future this function may perhaps
/// lock the entire directory to prevent this from happening, or maybe find the smallest free range
/// with the minimum alignment, to allow other drivers to obtain their necessary IRQs.
///
/// Note that this count/alignment restriction is only mandatory for MSI; MSI-X allows for
/// individually allocated vectors that might be spread out, even on multiple CPUs. Thus, multiple
/// invocations with alignment 1 and count 1 are totally acceptable, although allocating in bulk
/// minimizes the initialization overhead, even though it's negligible.
pub fn allocate_aligned_interrupt_vectors(alignment: NonZeroU8, count: u8) -> io::Result<Option<(u8, Vec<File>)>> {
    if count == 0 { return Ok(None) }

    let available_irqs = fs::read_dir("irq:")?;
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

        // note that there might be future subdirectories in the IRQ scheme, such as `cpu-<APIC
        // ID>/<IRQ>`, thus no error but just None
        match path_str.parse::<u8>() {
            Ok(p) => Some(Ok(p)),
            Err(_) => None,
        }
    });

    // TODO: fcntl F_SETLK on `irq:/`?

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
        let irq_handle = match File::create(format!("irq:{}", irq_number)) {
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
pub fn allocate_interrupt_vectors(count: u8) -> io::Result<Option<(u8, Vec<File>)>> {
    allocate_aligned_interrupt_vectors(NonZeroU8::new(1).unwrap(), count)
}

/// Allocate a single interrupt vector, returning both the vector number (starting from 32 up to
/// 254), and its IRQ handle which is then reserved. Returns Ok(None) if allocation fails due to
/// no available IRQs.
pub fn allocate_single_interrupt_vector() -> io::Result<Option<(u8, File)>> {
    let (base, mut files) = match allocate_interrupt_vectors(1) {
        Ok(Some((base, files))) => (base, files),
        Ok(None) => return Ok(None),
        Err(err) => return Err(err),
    };
    assert_eq!(files.len(), 1);
    Ok(Some((base, files.pop().unwrap())))
}
