use std::ptr::NonNull;
use std::sync::Mutex;

use syscall::{PHYSMAP_NO_CACHE, PHYSMAP_WRITE};

/// A wrapper for an RAII-based BAR allocation. Freed on drop, so it's recommended that these be
/// stored in `Arc<AllocatedBars>` for the lifetime of a driver.
pub struct Bar {
    ptr: NonNull<u8>,
    physical: usize,
    bar_size: usize,
}
impl Bar {
    /// Map the physical bar and corresponding bar size into virtual memory usable by this process.
    pub unsafe fn map(bar: usize, bar_size: usize) -> syscall::Result<Self> {
        Ok(Self {
            ptr: NonNull::new(
                syscall::physmap(bar, bar_size, PHYSMAP_NO_CACHE | PHYSMAP_WRITE)? as *mut u8,
            )
            .expect("Mapping a BAR resulted in a nullptr"),
            physical: bar,
            bar_size,
        })
    }
    /// The virtual address of the mapped BAR.
    pub fn pointer(&self) -> NonNull<u8> {
        self.ptr
    }
    /// The BAR size
    pub fn size(&self) -> usize {
        self.bar_size
    }
    /// Unmap the bar, doing the same thing as just dropping but doesn't silently drop any
    /// potential errors.
    pub fn unmap(self) -> syscall::Result<()> {
        unsafe {
            syscall::physunmap(self.physical)?;
        }
        // bypass drop, avoiding a double free
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        let _ = unsafe { syscall::physunmap(self.physical) };
    }
}

/// The PCI BARs that may be allocated.
#[derive(Default)]
pub struct AllocatedBars(pub [Mutex<Option<Bar>>; 6]);

/// IRQ helpers.
pub mod irq {
    //! IRQ helpers.
    //!
    //! This module allows easy handling of the `irq:` scheme, and allocating interrupt vectors for use
    //! by INTx#, MSI, or MSI-X.

    use std::collections::BTreeMap;
    use std::convert::TryFrom;
    use std::fs::{self, File};
    use std::io::{self, prelude::*};
    use std::num::NonZeroU8;
    use std::ptr::NonNull;
    use std::slice;

    use syscall::Mmio;

    use super::{
        super::{
            msi::{MsixCapability, MsixTableEntry},
            PciBar, PciFunction,
        },
        AllocatedBars, Bar,
    };

    /// Read the local APIC ID of the bootstrap processor.
    pub fn read_bsp_apic_id() -> io::Result<usize> {
        let mut buffer = [0u8; 8];

        let mut file = File::open("irq:bsp")?;
        let bytes_read = file.read(&mut buffer)?;

        (if bytes_read == 8 {
            usize::try_from(u64::from_le_bytes(buffer))
        } else if bytes_read == 4 {
            usize::try_from(u32::from_le_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ]))
        } else {
            panic!(
                "`irq:` scheme responded with {} bytes, expected {}",
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
            fs::read_dir("irq:")?.filter_map(|entry| -> Option<io::Result<_>> {
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

        let available_irqs = fs::read_dir(format!("irq:cpu-{:02x}", cpu_id))?;
        let mut available_irq_numbers =
            available_irqs.filter_map(|entry| -> Option<io::Result<_>> {
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
            let irq_handle = match File::create(format!("irq:cpu-{:02x}/{}", cpu_id, irq_number)) {
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
    pub fn allocate_interrupt_vectors(
        cpu_id: usize,
        count: u8,
    ) -> io::Result<Option<(u8, Vec<File>)>> {
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
    /// Retrieve slices to the entries and PBA array of an MSI-X capability struct.
    pub unsafe fn msix_cfg(
        function: &PciFunction,
        capability_struct: &MsixCapability,
        allocated_bars: &AllocatedBars,
    ) -> syscall::Result<(&'static mut [MsixTableEntry], &'static mut [Mmio<u64>])> {
        unsafe fn bar_base(
            allocated_bars: &AllocatedBars,
            function: &PciFunction,
            bir: u8,
        ) -> syscall::Result<NonNull<u8>> {
            let bir = usize::from(bir);
            let mut bar_guard = allocated_bars.0[bir].lock().unwrap();
            match &mut *bar_guard {
                &mut Some(ref bar) => Ok(bar.ptr),
                bar_to_set @ &mut None => {
                    let bar = match function.bars[bir].unwrap() {
                        PciBar::MemorySpace32 { address, .. } => address as usize,
                        PciBar::MemorySpace64 { address, .. } => address as usize,
                        other => panic!("Expected memory BAR, found {:?}", other),
                    };
                    let bar_size = function.bar_sizes[bir];

                    let bar = Bar::map(bar as usize, bar_size as usize)?;
                    *bar_to_set = Some(bar);
                    Ok(bar_to_set.as_ref().unwrap().ptr)
                }
            }
        }
        let table_bar_base: *mut u8 =
            bar_base(allocated_bars, function, capability_struct.table_bir())?.as_ptr();
        let pba_bar_base: *mut u8 =
            bar_base(allocated_bars, function, capability_struct.pba_bir())?.as_ptr();
        let table_base = table_bar_base.offset(capability_struct.table_offset() as isize);
        let pba_base = pba_bar_base.offset(capability_struct.pba_offset() as isize);

        let vector_count = capability_struct.table_size();
        let table_entries: &'static mut [MsixTableEntry] =
            slice::from_raw_parts_mut(table_base as *mut MsixTableEntry, vector_count as usize);
        let pba_entries: &'static mut [Mmio<u64>] = slice::from_raw_parts_mut(
            pba_base as *mut Mmio<u64>,
            (vector_count as usize + 63) / 64,
        );
        Ok((table_entries, pba_entries))
    }
    /// A an iterable collection of interrupt sources, either INTx#, MSI, or MSI-X.
    #[derive(Debug)]
    pub enum InterruptSources {
        /// MSI-X interrupt vectors, can be spread out arbitrarily.
        MsiX(BTreeMap<u16, File>),
        /// MSI interrupt vectors, guaranteed to be a power of two in length, and they start at 0.
        Msi(Vec<File>),
        /// A single interrupt vector at 0.
        Intx(File),
    }
    impl InterruptSources {
        /// An immutable iterator over the current vectors as `(u16, &File)`.
        pub fn iter(&self) -> impl Iterator<Item = (u16, &File)> {
            use std::collections::btree_map;
            use std::iter::{Enumerate, Once};

            enum Iter<'a> {
                MsiX(btree_map::Iter<'a, u16, File>),
                Msi(Enumerate<slice::Iter<'a, File>>),
                Intx(Once<&'a File>),
            }
            impl<'a> Iterator for Iter<'a> {
                type Item = (u16, &'a File);

                fn next(&mut self) -> Option<Self::Item> {
                    match self {
                        &mut Self::Msi(ref mut iter) => iter
                            .next()
                            .map(|(vector, handle)| (u16::try_from(vector).unwrap(), handle)),
                        &mut Self::MsiX(ref mut iter) => {
                            iter.next().map(|(&vector, handle)| (vector, handle))
                        }
                        &mut Self::Intx(ref mut iter) => iter.next().map(|handle| (0u16, handle)),
                    }
                }
                fn size_hint(&self) -> (usize, Option<usize>) {
                    match self {
                        &Self::Msi(ref iter) => iter.size_hint(),
                        &Self::MsiX(ref iter) => iter.size_hint(),
                        &Self::Intx(ref iter) => iter.size_hint(),
                    }
                }
            }

            match self {
                &Self::MsiX(ref map) => Iter::MsiX(map.iter()),
                &Self::Msi(ref vec) => Iter::Msi(vec.iter().enumerate()),
                &Self::Intx(ref single) => Iter::Intx(std::iter::once(single)),
            }
        }
        /// A mutable iterator over all of the current interrupt vectors as `(u16, &mut File)`.
        pub fn iter_mut(&mut self) -> impl Iterator<Item = (u16, &mut File)> {
            use std::collections::btree_map::IterMut as BTreeIterMut;
            use std::iter::{Enumerate, Once};

            enum IterMut<'a> {
                MsiX(BTreeIterMut<'a, u16, File>),
                Msi(Enumerate<slice::IterMut<'a, File>>),
                Intx(Once<&'a mut File>),
            }
            impl<'a> Iterator for IterMut<'a> {
                type Item = (u16, &'a mut File);

                fn next(&mut self) -> Option<Self::Item> {
                    match self {
                        &mut Self::Msi(ref mut iter) => iter
                            .next()
                            .map(|(vector, handle)| (u16::try_from(vector).unwrap(), handle)),
                        &mut Self::MsiX(ref mut iter) => {
                            iter.next().map(|(&vector, handle)| (vector, handle))
                        }
                        &mut Self::Intx(ref mut iter) => iter.next().map(|handle| (0u16, handle)),
                    }
                }
                fn size_hint(&self) -> (usize, Option<usize>) {
                    match self {
                        &Self::Msi(ref iter) => iter.size_hint(),
                        &Self::MsiX(ref iter) => iter.size_hint(),
                        &Self::Intx(ref iter) => iter.size_hint(),
                    }
                }
            }

            match self {
                &mut Self::MsiX(ref mut map) => IterMut::MsiX(map.iter_mut()),
                &mut Self::Msi(ref mut vec) => IterMut::Msi(vec.iter_mut().enumerate()),
                &mut Self::Intx(ref mut single) => IterMut::Intx(std::iter::once(single)),
            }
        }
        pub fn get(&self, vector: u16) -> Option<&File> {
            match self {
                &Self::Intx(ref handle) => {
                    if vector == 0 {
                        Some(handle)
                    } else {
                        None
                    }
                }
                &Self::Msi(ref vec) => vec.get(vector as usize),
                &Self::MsiX(ref map) => map.get(&vector),
            }
        }
        pub fn get_mut(&mut self, vector: u16) -> Option<&mut File> {
            match self {
                &mut Self::Intx(ref mut handle) => {
                    if vector == 0 {
                        Some(handle)
                    } else {
                        None
                    }
                }
                &mut Self::Msi(ref mut vec) => vec.get_mut(vector as usize),
                &mut Self::MsiX(ref mut map) => map.get_mut(&vector),
            }
        }
    }
}
