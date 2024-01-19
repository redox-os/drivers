use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::{fmt, fs, io, mem, ptr, slice};

use syscall::PAGE_SIZE;

use smallvec::{smallvec, SmallVec};

use crate::pci::{CfgAccess, Pci, PciAddress, PciIter};

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
        let len = total_length - mem::size_of::<Mcfg>();
        // safe because the length cannot be changed arbitrarily
        unsafe {
            slice::from_raw_parts(
                &self.base_addrs as *const PcieAlloc,
                len / mem::size_of::<PcieAlloc>(),
            )
        }
    }
}
impl fmt::Debug for Mcfg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Mcfg")
            .field("name", &self.name)
            .field("length", &{ self.length })
            .field("revision", &self.revision)
            .field("checksum", &self.checksum)
            .field("oem_id", &self.oem_id)
            .field("oem_table_id", &{ self.oem_table_id })
            .field("oem_revision", &{ self.oem_revision })
            .field("creator_revision", &{ self.creator_revision })
            .field("creator_id", &self.creator_id)
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
        self.tables()
            .map(|table| table.base_addr_structs().iter())
            .flatten()
    }

    pub fn fetch() -> io::Result<Self> {
        let table_dir = fs::read_dir("acpi:tables")?;

        let mut tables = smallvec![];
        for table_direntry in table_dir {
            let table_path = table_direntry?.path();

            // Every directory entry has to have a filename unless
            // the filesystem (or in this case acpid) misbehaves.
            // If it misbehaves we have worse problems than pcid
            // crashing. `as_encoded_bytes()` returns some superset
            // of ASCII, so directly comparing it with an ASCII name
            // is fine.
            let table_filename = table_path.file_name().unwrap().as_encoded_bytes();
            if table_filename.get(0..4) == Some(&MCFG_NAME) {
                tables.push(fs::read(table_path)?);
            }
        }

        Ok(Self { tables })
    }
    pub fn table_and_alloc_at_bus(&self, bus: u8) -> Option<(&Mcfg, &PcieAlloc)> {
        self.tables().find_map(|table| {
            Some((
                table,
                table.base_addr_structs().iter().find(|addr_struct| {
                    (addr_struct.start_bus..=addr_struct.end_bus).contains(&bus)
                })?,
            ))
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
    maps: Mutex<BTreeMap<PciAddress, *mut u32>>,
    fallback: Arc<Pci>,
}
unsafe impl Send for Pcie {}
unsafe impl Sync for Pcie {}

impl Pcie {
    pub fn new(fallback: Arc<Pci>) -> io::Result<Self> {
        let mcfgs = Mcfgs::fetch()?;

        Ok(Self {
            lock: Mutex::new(()),
            mcfgs,
            maps: Mutex::new(BTreeMap::new()),
            fallback,
        })
    }
    fn addr_offset_in_bytes(starting_bus: u8, address: PciAddress, offset: u16) -> usize {
        assert_eq!(offset & 0xFFFC, offset, "pcie offset not dword-aligned");
        assert_eq!(offset & 0x0FFF, offset, "pcie offset larger than 4095");

        assert_eq!(
            address.segment(),
            0,
            "multiple segments not yet implemented"
        );

        (((address.bus() - starting_bus) as usize) << 20)
            | ((address.device() as usize) << 15)
            | ((address.function() as usize) << 12)
            | (offset as usize)
    }
    fn addr_offset_in_dwords(starting_bus: u8, address: PciAddress, offset: u16) -> usize {
        Self::addr_offset_in_bytes(starting_bus, address, offset) / mem::size_of::<u32>()
    }
    unsafe fn with_pointer<T, F: FnOnce(Option<&mut u32>) -> T>(
        &self,
        address: PciAddress,
        offset: u16,
        f: F,
    ) -> T {
        let (base_address_phys, starting_bus) = match self.mcfgs.at_bus(address.bus()) {
            Some(t) => (t.base_addr, t.start_bus),
            None => return f(None),
        };
        let mut maps_lock = self.maps.lock().unwrap();
        let virt_pointer = maps_lock.entry(address).or_insert_with(|| {
            common::physmap(
                base_address_phys as usize + Self::addr_offset_in_bytes(starting_bus, address, 0),
                PAGE_SIZE,
                common::Prot {
                    read: true,
                    write: true,
                },
                common::MemoryType::Uncacheable,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "failed to physmap pcie configuration space for {}: {:?}",
                    address, error
                )
            }) as *mut u32
        });
        f(Some(&mut *virt_pointer.offset(
            (offset as usize / mem::size_of::<u32>()) as isize,
        )))
    }
    pub fn buses<'pcie>(&'pcie self) -> PciIter<'pcie> {
        PciIter::new(self)
    }
}

impl CfgAccess for Pcie {
    unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
        let _guard = self.lock.lock().unwrap();

        self.with_pointer(address, offset, |pointer| match pointer {
            Some(address) => ptr::read_volatile::<u32>(address),
            None => self.fallback.read(address, offset),
        })
    }

    unsafe fn write(&self, address: PciAddress, offset: u16, value: u32) {
        let _guard = self.lock.lock().unwrap();

        self.with_pointer(address, offset, |pointer| match pointer {
            Some(address) => ptr::write_volatile::<u32>(address, value),
            None => {
                self.fallback.write(address, offset, value);
            }
        });
    }
}

impl Drop for Pcie {
    fn drop(&mut self) {
        for address in self.maps.lock().unwrap().values().copied() {
            let _ = unsafe { syscall::funmap(address as usize, PAGE_SIZE) };
        }
    }
}
