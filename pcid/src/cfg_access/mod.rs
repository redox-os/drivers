use std::sync::Mutex;
use std::{fmt, fs, io, mem, ptr};

use common::{MemoryType, PhysBorrowed, Prot};
use pci_types::{ConfigRegionAccess, PciAddress};

use fallback::Pci;

mod fallback;

pub const MCFG_NAME: [u8; 4] = *b"MCFG";

#[repr(packed)]
#[derive(Clone, Copy, Debug)]
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

#[derive(Debug)]
struct PcieAllocs<'a>(&'a [PcieAlloc]);

impl Mcfg {
    fn with<T>(f: impl FnOnce(&Mcfg, PcieAllocs<'_>) -> io::Result<T>) -> io::Result<T> {
        let table_dir = fs::read_dir("acpi:tables")?;

        // TODO: validate/print MCFG?

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
                    Some((mcfg, allocs)) => return f(mcfg, allocs),
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

    fn parse<'a>(bytes: &'a [u8]) -> Option<(&'a Mcfg, PcieAllocs<'a>)> {
        if bytes.len() < mem::size_of::<Mcfg>() {
            return None;
        }
        let (header_bytes, allocs_bytes) = bytes.split_at(mem::size_of::<Mcfg>());

        let mcfg =
            plain::from_bytes::<Mcfg>(header_bytes).expect("packed -> align 1, checked size");
        if mcfg.length as usize != bytes.len() {
            log::warn!("MCFG {mcfg:?} length mismatch, expected {}", bytes.len());
            return None;
        }
        // TODO: Allow invalid bytes not divisible by PcieAlloc?

        let allocs_len =
            allocs_bytes.len() / mem::size_of::<PcieAlloc>() * mem::size_of::<PcieAlloc>();

        let allocs = plain::slice_from_bytes::<PcieAlloc>(&allocs_bytes[..allocs_len])
            .expect("packed -> align 1, checked size");
        Some((mcfg, PcieAllocs(allocs)))
    }
}

pub struct Pcie {
    lock: Mutex<()>,
    allocs: Vec<Alloc>,
    fallback: Pci,
}
struct Alloc {
    seg: u16,
    start_bus: u8,
    end_bus: u8,
    mem: PhysBorrowed,
}

unsafe impl Send for Pcie {}
unsafe impl Sync for Pcie {}

const BYTES_PER_BUS: usize = 1 << 20;

impl Pcie {
    pub fn new() -> Self {
        match Mcfg::with(|mcfg, allocs| {
            log::info!("MCFG {mcfg:?} ALLOCS {allocs:?}");
            let mut allocs = allocs
                .0
                .iter()
                .filter_map(|desc| {
                    Some(Alloc {
                        seg: desc.seg_group_num,
                        start_bus: desc.start_bus,
                        end_bus: desc.end_bus,
                        mem: PhysBorrowed::map(
                            desc.base_addr.try_into().ok()?,
                            BYTES_PER_BUS
                                * (usize::from(desc.end_bus) - usize::from(desc.start_bus) + 1),
                            Prot::RW,
                            MemoryType::Uncacheable,
                        )
                        .inspect_err(|err| {
                            log::error!(
                                "failed to map seg {} bus {}..={}: {}",
                                { desc.seg_group_num },
                                { desc.start_bus },
                                { desc.end_bus },
                                err
                            )
                        })
                        .ok()?,
                    })
                })
                .collect::<Vec<_>>();

            allocs.sort_by_key(|alloc| (alloc.seg, alloc.start_bus));

            Ok(Self {
                lock: Mutex::new(()),
                allocs,
                fallback: Pci::new(),
            })
        }) {
            Ok(pcie) => pcie,
            Err(error) => {
                log::warn!("Couldn't retrieve PCIe info, perhaps the kernel is not compiled with acpi? Using the PCI 3.0 configuration space instead. Error: {:?}", error);
                Self {
                    lock: Mutex::new(()),
                    allocs: Vec::new(),
                    fallback: Pci::new(),
                }
            }
        }
    }
    fn bus_addr(&self, seg: u16, bus: u8) -> Option<*mut u32> {
        let alloc = match self
            .allocs
            .binary_search_by_key(&(seg, bus), |alloc| (alloc.seg, alloc.start_bus))
        {
            Ok(present_idx) => &self.allocs[present_idx],
            Err(0) => return None,
            Err(above_idx) => {
                let below_alloc = &self.allocs[above_idx - 1];
                if bus > below_alloc.end_bus {
                    return None;
                }
                below_alloc
            }
        };
        let bus_off = bus - alloc.start_bus;
        Some(unsafe {
            alloc
                .mem
                .as_ptr()
                .cast::<u8>()
                .add(usize::from(bus_off) * BYTES_PER_BUS)
                .cast::<u32>()
        })
    }

    fn bus_addr_offset_in_dwords(address: PciAddress, offset: u16) -> usize {
        assert_eq!(offset & 0xFFFC, offset, "pcie offset not dword-aligned");
        assert_eq!(offset & 0x0FFF, offset, "pcie offset larger than 4095");

        (((address.device() as usize) << 15)
            | ((address.function() as usize) << 12)
            | (offset as usize))
            >> 2
    }
    // TODO: A safer interface, using e.g. a VolatileCell or Volatile<'a>. The PhysBorrowed wrapper
    // can possibly deref to or provide a Volatile<T>.
    fn mmio_addr(&self, address: PciAddress, offset: u16) -> Option<*mut u32> {
        assert_eq!(
            address.segment(),
            0,
            "multiple segments not yet implemented"
        );

        assert_eq!(offset & 0xFC, offset, "pci offset is not aligned");

        let bus_addr = self.bus_addr(address.segment(), address.bus())?;
        Some(unsafe { bus_addr.add(Self::bus_addr_offset_in_dwords(address, offset)) })
    }
}

impl ConfigRegionAccess for Pcie {
    fn function_exists(&self, _address: PciAddress) -> bool {
        todo!();
    }

    unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
        let _guard = self.lock.lock().unwrap();

        match self.mmio_addr(address, offset) {
            Some(addr) => addr.read_volatile(),
            None => self.fallback.read(address, offset),
        }
    }

    unsafe fn write(&self, address: PciAddress, offset: u16, value: u32) {
        let _guard = self.lock.lock().unwrap();

        match self.mmio_addr(address, offset) {
            Some(addr) => addr.write_volatile(value),
            None => self.fallback.write(address, offset, value),
        }
    }
}
