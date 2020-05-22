use std::{fmt, fs, io, mem, ptr, slice};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use syscall::flag::PhysmapFlags;
use syscall::io::{Dma, Io, Mmio};

use smallvec::SmallVec;

use crate::pci::{CfgAccess, Pci, PciIter};

pub mod cap;

pub const MCFG_NAME: [u8; 4] = *b"MCFG";

#[repr(packed)]
#[derive(Clone, Copy)]
pub struct Mcfg {
    // base sdt fields
    name: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: [u8; 4],
    creator_revision: u32,
    _rsvd: [u8; 8],

    base_addrs: [PcieAlloc; 0],
}
unsafe impl plain::Plain for Mcfg {}

/// The "Memory Mapped Enhanced Configuration Space Base Address Allocation Structure" (yes, it's
/// called that).

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
pub struct PcieAlloc {
    pub base_addr: u64,
    pub seg_group_num: u16,
    pub start_bus: u8,
    pub end_bus: u8,
    _rsvd: [u8; 4],
}
unsafe impl plain::Plain for PcieAlloc {}

impl Mcfg {
    pub fn base_addr_structs(&self) -> &[PcieAlloc] {
        let total_length = self.length as usize;
        assert!(total_length > mem::size_of::<Self>());
        let len = total_length - 44;
        // safe because the length cannot be changed arbitrarily
        unsafe { slice::from_raw_parts(&self.base_addrs as *const PcieAlloc, len / mem::size_of::<PcieAlloc>()) }
    }
}
impl fmt::Debug for Mcfg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Mcfg")
            .field("name", &"MCFG")
            .field("length", &{self.length})
            .field("revision", &{self.revision})
            .field("checksum", &{self.checksum})
            .field("oem_id", &{self.oem_id})
            .field("oem_table_id", &{self.oem_table_id})
            .field("oem_revision", &{self.oem_revision})
            .field("creator_revision", &{self.creator_revision})
            .field("creator_id", &{self.creator_id})
            .field("base_addrs", &self.base_addr_structs())
            .finish()
    }
}

pub struct Mcfgs {
    tables: SmallVec<[Vec<u8>; 2]>,
}

impl Mcfgs {
    pub fn tables<'a>(&'a self) -> impl Iterator<Item = &'a Mcfg> + 'a {
        self.tables.iter().filter_map(|bytes| {
            let mcfg = plain::from_bytes::<Mcfg>(bytes).ok()?;
            if mcfg.length as usize > bytes.len() {
                return None;
            }
            Some(mcfg)
        })
    }
    pub fn allocs<'a>(&'a self) -> impl Iterator<Item = &'a PcieAlloc> + 'a {
        self.tables().map(|table| table.base_addr_structs().iter()).flatten()
    }

    pub fn fetch() -> io::Result<Self> {
        log::debug!("Fetching MCFG table.");
        let table_dir = fs::read_dir("acpi:tables")?;

        let tables = table_dir.map(|table_direntry| -> io::Result<Option<_>> {
            let table_direntry = table_direntry?;
            let table_path = table_direntry.path();

            let table_filename = match table_path.file_name() {
                Some(n) => n.to_str().ok_or(io::Error::new(io::ErrorKind::InvalidData, "Non-UTF-8 ACPI table filename"))?,
                None => return Ok(None),
            };

            if table_filename.starts_with("MCFG") {
                log::debug!("Found table: {}", &*table_path.to_string_lossy());
                Ok(Some(fs::read(table_path)?))
            } else {
                Ok(None)
            }
        }).filter_map(|result_option| result_option.transpose()).collect::<Result<SmallVec<_>, _>>()?;

        Ok(Self {
            tables,
        })
    }
    pub fn table_and_alloc_at_bus(&self, bus: u8) -> Option<(&Mcfg, &PcieAlloc)> {
        self.tables().find_map(|table| {
            Some((table, table.base_addr_structs().iter().find(|addr_struct| {
                (addr_struct.start_bus..addr_struct.end_bus).contains(&bus)
            })?))
        })
    }
    pub fn at_bus(&self, bus: u8) -> Option<&PcieAlloc> {
        self.table_and_alloc_at_bus(bus).map(|(_, alloc)| alloc)
    }
}

impl fmt::Debug for Mcfgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct Tables<'a>(&'a Mcfgs);
        impl<'a> fmt::Debug for Tables<'a> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.debug_list().entries(self.0.tables()).finish()
            }
        }

        f.debug_tuple("Mcfgs").field(&Tables(self)).finish()
    }
}

pub struct Pcie {
    lock: Mutex<()>,
    mcfgs: Mcfgs,
    maps: Mutex<BTreeMap<(u8, u8, u8), *mut u32>>,
    fallback: Arc<Pci>,
}
unsafe impl Send for Pcie {}
unsafe impl Sync for Pcie {}

impl Pcie {
    pub fn new(fallback: Arc<Pci>) -> io::Result<Self> {
        let mcfgs = Mcfgs::fetch()?;

        log::debug!("Found MCFG tables: {:?}", mcfgs.tables().collect::<Vec<_>>());

        Ok(Self {
            lock: Mutex::new(()),
            mcfgs,
            maps: Mutex::new(BTreeMap::new()),
            fallback,
        })
    }
    fn addr_offset_in_bytes(starting_bus: u8, bus: u8, dev: u8, func: u8, offset: u16) -> usize {
        assert_eq!(offset & 0xFFFC, offset, "pcie offset not dword-aligned");
        assert_eq!(offset & 0x0FFF, offset, "pcie offset larger than 4095");
        assert_eq!(dev & 0x1F, dev, "pcie dev number larger than 5 bits");
        assert_eq!(func & 0x7, func, "pcie func number larger than 3 bits");

        (((bus - starting_bus) as usize) << 20) | ((dev as usize) << 15) | ((func as usize) << 12) | (offset as usize)
    }
    fn addr_offset_in_dwords(starting_bus: u8, bus: u8, dev: u8, func: u8, offset: u16) -> usize {
        Self::addr_offset_in_bytes(starting_bus, bus, dev, func, offset) / mem::size_of::<u32>()
    }
    unsafe fn with_pointer<T, F: FnOnce(Option<&mut Mmio<u32>>) -> T>(&self, bus: u8, dev: u8, func: u8, offset: u16, f: F) -> T {
        assert!(offset < 4096);

        self.with_slice(bus, dev, func, move |slice: Option<&'static mut [Mmio<u32>]>| {
            f(slice.map(|slice| &mut slice[offset as usize / mem::size_of::<Mmio<u32>>()]))
        })
    }
    unsafe fn with_func_mem<T, F: FnOnce(Option<*mut u32>) -> T>(&self, bus: u8, dev: u8, func: u8, f: F) -> T {
        let (base_address_phys, starting_bus) = match self.mcfgs.at_bus(bus) {
            Some(t) => (t.base_addr, t.start_bus),
            None => return f(None),
        };
        let physical = base_address_phys as usize + Self::addr_offset_in_bytes(starting_bus, bus, dev, func, 0);
        let mut maps_lock = self.maps.lock().unwrap();
        let virt_pointer: *mut u32 = *maps_lock.entry((bus, dev, func)).or_insert_with(|| {
            syscall::physmap(physical, 4096, PhysmapFlags::PHYSMAP_NO_CACHE | PhysmapFlags::PHYSMAP_WRITE).unwrap_or_else(|error| panic!("failed to physmap pcie configuration space for {:2x}:{:2x}.{:2x}: {:?}", bus, dev, func, error)) as *mut u32
        });
        log::trace!("PCIe func {}:{}.{} has physaddr {:p} and virtaddr {:p}", bus, dev, func, physical as *mut u32, virt_pointer as *mut u32);
        f(Some(virt_pointer))
    }
    unsafe fn with_slice<T, F: FnOnce(Option<&'static mut [Mmio<u32>]>) -> T>(&self, bus: u8, dev: u8, func: u8, f: F) -> T {
        self.with_func_mem(bus, dev, func, move |func_base_ptr| {
            f(func_base_ptr.map(|ptr| {
                slice::from_raw_parts_mut(ptr as *mut Mmio<u32>, 4096 / mem::size_of::<Mmio<u32>>())
            }))
        })
    }
    pub fn buses<'pcie>(&'pcie self) -> PciIter<'pcie> {
        PciIter::new(self)
    }
}

impl CfgAccess for Pcie {
    fn supports_ext(&self, bus: u8) -> bool {
        self.mcfgs.at_bus(bus).is_some()
    }

    unsafe fn read_nolock(&self, bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
        assert!(offset < 4096);

        self.with_func_mem(bus, dev, func, |pointer| match pointer {
            Some(address) => std::ptr::read_volatile(address.add(offset as usize / 4)),
            None => self.fallback.read(bus, dev, func, offset),
        })
    }
    unsafe fn read(&self, bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
        let _guard = self.lock.lock().unwrap();
        self.read_nolock(bus, dev, func, offset)
    }
    unsafe fn write_nolock(&self, bus: u8, dev: u8, func: u8, offset: u16, value: u32) {
        assert!(offset < 4096);

        self.with_func_mem(bus, dev, func, |pointer| match pointer {
            Some(address) => std::ptr::write_volatile(address.add(offset as usize / 4), value),
            None => { self.fallback.read(bus, dev, func, offset); }
        });
    }
    unsafe fn write(&self, bus: u8, dev: u8, func: u8, offset: u16, value: u32) {
        let _guard = self.lock.lock().unwrap();
        self.write_nolock(bus, dev, func, offset, value);
    }
    unsafe fn with_mapped_mem(&self, bus: u8, dev: u8, func: u8, f: &mut dyn FnMut(Option<&'static mut [Mmio<u32>]>)) {
        self.with_slice(bus, dev, func, f);
    }
}

impl Drop for Pcie {
    fn drop(&mut self) {
        for address in self.maps.lock().unwrap().values().copied() {
            let _ = unsafe { syscall::physfree(address as usize, 4096) };
        }
    }
}
