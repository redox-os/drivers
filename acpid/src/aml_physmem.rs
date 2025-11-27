use acpi::{aml::AmlError, Handle, PciAddress, PhysicalMapping};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use common::io::{Io, Pio};
use num_traits::PrimInt;
use rustc_hash::FxHashMap;
use std::fmt::LowerHex;
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use syscall::PAGE_SIZE;

const PAGE_MASK: usize = !(PAGE_SIZE - 1);
const OFFSET_MASK: usize = PAGE_SIZE - 1;

struct MappedPage {
    phys_page: usize,
    virt_page: usize,
}

impl MappedPage {
    fn new(phys_page: usize) -> std::io::Result<Self> {
        let virt_page = unsafe {
            common::physmap(
                phys_page,
                PAGE_SIZE,
                common::Prot::RW,
                common::MemoryType::default(),
            )
            .map_err(|error| std::io::Error::from_raw_os_error(error.errno()))?
        } as usize;
        Ok(Self {
            phys_page,
            virt_page,
        })
    }
}

impl Drop for MappedPage {
    fn drop(&mut self) {
        log::trace!("Drop page {:#x}", self.phys_page);
        if let Err(e) = unsafe { libredox::call::munmap(self.virt_page as *mut (), PAGE_SIZE) } {
            log::error!("funmap (phys): {:?}", e);
        }
    }
}

#[derive(Default)]
pub struct AmlPageCache {
    page_cache: FxHashMap<usize, MappedPage>,
}

impl AmlPageCache {
    /// get a virtual address for the given physical page
    fn get_page(&mut self, phys_target: usize) -> std::io::Result<&MappedPage> {
        let phys_page = phys_target & PAGE_MASK;
        if self.page_cache.contains_key(&phys_page) {
            log::trace!("re-using cached page {:#x}", phys_page);

            Ok(self
                .page_cache
                .get(&phys_page)
                .expect("could not get page after contains=true"))
        } else {
            let mapped_page = MappedPage::new(phys_page)?;
            log::trace!("adding page {:#x} to cache", mapped_page.phys_page);
            self.page_cache.insert(phys_page, mapped_page);
            Ok(self
                .page_cache
                .get(&phys_page)
                .expect("can't find page that was just inserted"))
        }
    }

    /// The offset into the virtual slice of T that matches the physical target
    fn sized_index<T>(phys_target: usize) -> usize {
        assert_eq!(
            phys_target & !(size_of::<T>() - 1),
            phys_target,
            "address {} is not aligned",
            phys_target
        );
        (phys_target & OFFSET_MASK) / size_of::<T>()
    }
    /// Read from the given physical address
    fn read_from_phys<T: PrimInt + LowerHex>(&mut self, phys_target: usize) -> std::io::Result<T> {
        let mapped_page = self.get_page(phys_target)?;
        let page_as_slice = unsafe {
            std::slice::from_raw_parts(
                mapped_page.virt_page as *const T,
                PAGE_SIZE / size_of::<T>(),
            )
        };
        // for debugging only
        let _virt_ptr = page_as_slice[Self::sized_index::<T>(phys_target)..].as_ptr() as usize;

        let val = page_as_slice[Self::sized_index::<T>(phys_target)];

        log::trace!(
            "read {:#x}, virt {:#x}, val {:#x}",
            phys_target,
            _virt_ptr,
            val
        );
        Ok(val)
    }

    /// Write to the given physical address
    fn write_to_phys<T: PrimInt + LowerHex>(
        &mut self,
        phys_target: usize,
        val: T,
    ) -> std::io::Result<()> {
        let mapped_page = self.get_page(phys_target)?;
        let page_as_slice = unsafe {
            std::slice::from_raw_parts_mut(
                mapped_page.virt_page as *mut T,
                PAGE_SIZE / size_of::<T>(),
            )
        };
        // for debugging only
        let _virt_ptr = page_as_slice[Self::sized_index::<T>(phys_target)..].as_ptr() as usize;

        page_as_slice[Self::sized_index::<T>(phys_target)] = val;

        log::trace!(
            "write {:#x}, virt {:#x}, val {:#x}",
            phys_target,
            _virt_ptr,
            val
        );
        Ok(())
    }

    pub fn clear(&mut self) {
        log::trace!("Clear page cache");
        self.page_cache.clear();
    }
}

#[derive(Clone)]
pub struct AmlPhysMemHandler {
    page_cache: Arc<Mutex<AmlPageCache>>,
    pci_fd: Arc<Option<libredox::Fd>>,
}

/// Read from a physical address.
/// Generic parameter must be u8, u16, u32 or u64.
impl AmlPhysMemHandler {
    pub fn new(page_cache: Arc<Mutex<AmlPageCache>>) -> Self {
        //TODO: have PCID send a socket?
        let pci_fd = Arc::new(
            match libredox::Fd::open(
                "/scheme/pci/access",
                libredox::flag::O_RDWR | libredox::flag::O_CLOEXEC,
                0
            ) {
                Ok(fd) => Some(fd),
                Err(err) => {
                    log::error!("failed to open /scheme/pci/access: {}", err);
                    None
                }
            }
        );
        Self { page_cache, pci_fd }
    }

    fn pci_call_metadata(kind: u8, addr: PciAddress, off: u16) -> [u64; 2] {
        // Segment: u16, at 28 bits
        // Bus: u8, 8 bits, 256 total, at 20 bits
        // Device: u8, 5 bits, 32 total, at 15 bits
        // Function: u8, 3 bits, 8 total, at 12 bits
        // Offset: u16, 12 bits, 4096 total, at 0 bits
        [
            kind.into(),
            (u64::from(addr.segment()) << 28) |
            (u64::from(addr.bus()) << 20) |
            (u64::from(addr.device()) << 15) |
            (u64::from(addr.function()) << 12) |
            u64::from(off)
        ]
    }

    fn read_pci(&self, addr: PciAddress, off: u16, value: &mut [u8]) {
        let metadata = Self::pci_call_metadata(1, addr, off);
        match &*self.pci_fd {
            Some(pci_fd) => match pci_fd.call_ro(value, syscall::CallFlags::empty(), &metadata) {
                Ok(_) => {},
                Err(err) => {
                    log::error!("read pci {addr}@{off:04X}:{:02X}: {}", value.len(), err);
                }
            },
            None => {
                log::error!("read pci {addr}@{off:04X}:{:02X}: pci access not available", value.len());
            }
        }
    }

    fn write_pci(&self, addr: PciAddress, off: u16, value: &[u8]) {
        let metadata = Self::pci_call_metadata(2, addr, off);
        match &*self.pci_fd {
            Some(pci_fd) => match pci_fd.call_wo(value, syscall::CallFlags::empty(), &metadata) {
                Ok(_) => {},
                Err(err) => {
                    log::error!("write pci {addr}@{off:04X}={value:02X?}: {}", err);
                }
            }
            None => {
                log::error!("write pci {addr}@{off:04X}={value:02X?}: pci access not available");
            }
        }
    }
}

impl acpi::Handler for AmlPhysMemHandler {
    unsafe fn map_physical_region<T>(&self, phys: usize, size: usize) -> PhysicalMapping<Self, T> {
        let phys_page = phys & PAGE_MASK;
        let offset = phys & OFFSET_MASK;
        let pages = (offset + size + PAGE_SIZE - 1) / PAGE_SIZE;
        let map_size = pages * PAGE_SIZE;
        let virt_page = common::physmap(
            phys_page,
            map_size,
            common::Prot::RW,
            common::MemoryType::default(),
        )
        .expect("failed to map physical region") as usize;
        PhysicalMapping {
            physical_start: phys,
            virtual_start: NonNull::new((virt_page + offset) as *mut T).unwrap(),
            region_length: size,
            mapped_length: map_size,
            handler: self.clone(),
        }
    }
    fn unmap_physical_region<T>(region: &PhysicalMapping<Self, T>) {
        let virt_page = region.virtual_start.addr().get() & PAGE_MASK;
        unsafe {
            libredox::call::munmap(virt_page as *mut (), region.mapped_length)
                .expect("failed to unmap physical region")
        }
    }

    fn read_u8(&self, address: usize) -> u8 {
        log::trace!("read u8 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u8>(address) {
                return value;
            }
        }
        log::error!("failed to read u8 {:#x}", address);
        0
    }
    fn read_u16(&self, address: usize) -> u16 {
        log::trace!("read u16 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u16>(address) {
                return value;
            }
        }
        log::error!("failed to read u16 {:#x}", address);
        0
    }
    fn read_u32(&self, address: usize) -> u32 {
        log::trace!("read u32 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u32>(address) {
                return value;
            }
        }
        log::error!("failed to read u32 {:#x}", address);
        0
    }
    fn read_u64(&self, address: usize) -> u64 {
        log::trace!("read u64 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u64>(address) {
                return value;
            }
        }
        log::error!("failed to read u64 {:#x}", address);
        0
    }

    fn write_u8(&self, address: usize, value: u8) {
        log::trace!("write u8 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u8>(address, value).is_ok() {
                return;
            }
        }
        log::error!("failed to write u8 {:#x}", address);
    }
    fn write_u16(&self, address: usize, value: u16) {
        log::trace!("write u16 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u16>(address, value).is_ok() {
                return;
            }
        }
        log::error!("failed to write u16 {:#x}", address);
    }
    fn write_u32(&self, address: usize, value: u32) {
        log::trace!("write u32 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u32>(address, value).is_ok() {
                return;
            }
        }
        log::error!("failed to write u32 {:#x}", address);
    }
    fn write_u64(&self, address: usize, value: u64) {
        log::trace!("write u64 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u64>(address, value).is_ok() {
                return;
            }
        }
        log::error!("failed to write u64 {:#x}", address);
    }

    // Pio must be enabled via syscall::iopl
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn read_io_u8(&self, port: u16) -> u8 {
        Pio::<u8>::new(port).read()
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn read_io_u16(&self, port: u16) -> u16 {
        Pio::<u16>::new(port).read()
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn read_io_u32(&self, port: u16) -> u32 {
        Pio::<u32>::new(port).read()
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn write_io_u8(&self, port: u16, value: u8) {
        Pio::<u8>::new(port).write(value)
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn write_io_u16(&self, port: u16, value: u16) {
        Pio::<u16>::new(port).write(value)
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn write_io_u32(&self, port: u16, value: u32) {
        Pio::<u32>::new(port).write(value)
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn read_io_u8(&self, port: u16) -> u8 {
        log::error!("cannot read u8 from port 0x{port:04X}");
        0
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn read_io_u16(&self, port: u16) -> u16 {
        log::error!("cannot read u16 from port 0x{port:04X}");
        0
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn read_io_u32(&self, port: u16) -> u32 {
        log::error!("cannot read u32 from port 0x{port:04X}");
        0
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn write_io_u8(&self, port: u16, value: u8) {
        log::error!("cannot write 0x{value:02X} to port 0x{port:04X}");
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn write_io_u16(&self, port: u16, value: u16) {
        log::error!("cannot write 0x{value:04X} to port 0x{port:04X}");
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn write_io_u32(&self, port: u16, value: u32) {
        log::error!("cannot write 0x{value:08X} to port 0x{port:04X}");
    }

    fn read_pci_u8(&self, addr: PciAddress, off: u16) -> u8 {
        let mut value = [0u8];
        self.read_pci(addr, off, &mut value);
        value[0]
    }
    fn read_pci_u16(&self, addr: PciAddress, off: u16) -> u16 {
        let mut value = [0u8; 2];
        self.read_pci(addr, off, &mut value);
        u16::from_le_bytes(value)
    }
    fn read_pci_u32(&self, addr: PciAddress, off: u16) -> u32 {
        let mut value = [0u8; 4];
        self.read_pci(addr, off, &mut value);
        u32::from_le_bytes(value)
    }
    fn write_pci_u8(&self, addr: PciAddress, off: u16, value: u8) {
        self.write_pci(addr, off, &[value]);
    }
    fn write_pci_u16(&self, addr: PciAddress, off: u16, value: u16) {
        self.write_pci(addr, off, &value.to_le_bytes());
    }
    fn write_pci_u32(&self, addr: PciAddress, off: u16, value: u32) {
        self.write_pci(addr, off, &value.to_le_bytes());
    }

    fn nanos_since_boot(&self) -> u64 {
        let ts = libredox::call::clock_gettime(libredox::flag::CLOCK_MONOTONIC)
            .expect("failed to get time");
        (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
    }

    fn stall(&self, microseconds: u64) {
        let start = std::time::Instant::now();
        while start.elapsed().as_micros() < microseconds.into() {
            std::hint::spin_loop();
        }
    }

    fn sleep(&self, milliseconds: u64) {
        std::thread::sleep(std::time::Duration::from_millis(milliseconds));
    }

    fn create_mutex(&self) -> Handle {
        log::debug!("TODO: Handler::create_mutex");
        Handle(0)
    }

    fn acquire(&self, mutex: Handle, timeout: u16) -> Result<(), AmlError> {
        log::debug!("TODO: Handler::acquire");
        Ok(())
    }

    fn release(&self, mutex: Handle) {
        log::debug!("TODO: Handler::release");
    }
}
