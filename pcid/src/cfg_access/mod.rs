use std::sync::Mutex;
use std::{fmt, fs, io, mem, ptr, slice};

use log::info;
use pci_types::{ConfigRegionAccess, PciAddress};

use fallback::Pci;

mod fallback;

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
    fn with<T>(f: impl FnOnce(&Mcfg) -> io::Result<T>) -> io::Result<T> {
        let table_dir = fs::read_dir("acpi:tables")?;

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
                let bytes = fs::read(table_path)?.into_boxed_slice();
                match Mcfg::parse(&*bytes) {
                    Some(mcfg) => return f(mcfg),
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "couldn't find mcfg table",
                        ));
                    }
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "couldn't find mcfg table",
        ))
    }

    fn parse<'a>(bytes: &'a [u8]) -> Option<&'a Mcfg> {
        let mcfg = plain::from_bytes::<Mcfg>(bytes).ok()?;
        if mcfg.length as usize > bytes.len() {
            return None;
        }
        Some(mcfg)
    }

    fn at_bus(&self, bus: u8) -> Option<&PcieAlloc> {
        Some(
            self.base_addr_structs()
                .iter()
                .find(|addr_struct| (addr_struct.start_bus..=addr_struct.end_bus).contains(&bus))?,
        )
    }

    fn base_addr_structs(&self) -> &[PcieAlloc] {
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

pub struct Pcie {
    lock: Mutex<()>,
    bus_maps: Vec<Option<(*mut u32, usize)>>,
    fallback: Pci,
}
unsafe impl Send for Pcie {}
unsafe impl Sync for Pcie {}

impl Pcie {
    pub fn new() -> Self {
        match Mcfg::with(|mcfg| {
            let alloc_maps = (0..=255)
                .map(|bus| {
                    if let Some(alloc) = mcfg.at_bus(bus) {
                        Some(unsafe { Self::physmap_pcie_bus(alloc, bus) })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            Ok(Self {
                lock: Mutex::new(()),
                bus_maps: alloc_maps,
                fallback: Pci::new(),
            })
        }) {
            Ok(pcie) => pcie,
            Err(error) => {
                info!("Couldn't retrieve PCIe info, perhaps the kernel is not compiled with acpi? Using the PCI 3.0 configuration space instead. Error: {:?}", error);
                Self {
                    lock: Mutex::new(()),
                    bus_maps: vec![],
                    fallback: Pci::new(),
                }
            }
        }
    }

    unsafe fn physmap_pcie_bus(alloc: &PcieAlloc, bus: u8) -> (*mut u32, usize) {
        let base_phys = alloc.base_addr as usize + (((bus - alloc.start_bus) as usize) << 20);
        let map_size = 1 << 20;
        let ptr = common::physmap(
            base_phys,
            map_size,
            common::Prot {
                read: true,
                write: true,
            },
            common::MemoryType::Uncacheable,
        )
        .unwrap_or_else(|error| {
            panic!(
                "failed to physmap pcie configuration space for segment {} bus {} @ {:p}: {:?}",
                { alloc.seg_group_num },
                bus,
                base_phys as *const u32,
                error,
            )
        }) as *mut u32;
        (ptr, map_size)
    }

    fn bus_addr_offset_in_bytes(address: PciAddress, offset: u16) -> usize {
        assert_eq!(offset & 0xFFFC, offset, "pcie offset not dword-aligned");
        assert_eq!(offset & 0x0FFF, offset, "pcie offset larger than 4095");

        ((address.device() as usize) << 15)
            | ((address.function() as usize) << 12)
            | (offset as usize)
    }
    unsafe fn with_pointer<T, F: FnOnce(Option<&mut u32>) -> T>(
        &self,
        address: PciAddress,
        offset: u16,
        f: F,
    ) -> T {
        assert_eq!(
            address.segment(),
            0,
            "multiple segments not yet implemented"
        );

        assert_eq!(offset & 0xFC, offset, "pci offset is not aligned");

        let bus_addr = match self.bus_maps.get(address.bus() as usize) {
            Some(Some(bus_addr)) => bus_addr,
            Some(None) | None => return f(None),
        };
        let virt_pointer = unsafe {
            // FIXME use byte_add once stable
            (bus_addr.0 as *mut u8).add(Self::bus_addr_offset_in_bytes(address, 0)) as *mut u32
        };
        f(Some(&mut *virt_pointer.offset(
            (offset as usize / mem::size_of::<u32>()) as isize,
        )))
    }
}

impl ConfigRegionAccess for Pcie {
    fn function_exists(&self, _address: PciAddress) -> bool {
        todo!();
    }

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
        for &map in &self.bus_maps {
            if let Some((ptr, size)) = map {
                let _ = unsafe { syscall::funmap(ptr as usize, size) };
            }
        }
    }
}
