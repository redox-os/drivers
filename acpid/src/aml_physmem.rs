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
}

/// Read from a physical address.
/// Generic parameter must be u8, u16, u32 or u64.
impl AmlPhysMemHandler {
    pub fn new(page_cache: Arc<Mutex<AmlPageCache>>) -> Self {
        Self { page_cache }
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
        log::error!("read pci u8 {addr}@{off:04X}");
        0
    }
    fn read_pci_u16(&self, addr: PciAddress, off: u16) -> u16 {
        log::error!("read pci u16 {addr}@{off:04X}");
        0
    }
    fn read_pci_u32(&self, addr: PciAddress, off: u16) -> u32 {
        log::error!("read pci u32 {addr}@{off:04X}");
        0
    }
    fn write_pci_u8(&self, addr: PciAddress, off: u16, value: u8) {
        log::error!("write pci u8 {addr}@{off:04X}={value:02X}");
    }
    fn write_pci_u16(&self, addr: PciAddress, off: u16, value: u16) {
        log::error!("write pci u16 {addr}@{off:04X}={value:04X}");
    }
    fn write_pci_u32(&self, addr: PciAddress, off: u16, value: u32) {
        log::error!("write pci u32 {addr}@{off:04X}={value:08X}");
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
        log::warn!("TODO: Handler::create_mutex");
        Handle(0)
    }

    fn acquire(&self, mutex: Handle, timeout: u16) -> Result<(), AmlError> {
        log::warn!("TODO: Handler::aquire");
        Ok(())
    }

    fn release(&self, mutex: Handle) {
        log::warn!("TODO: Handler::release");
    }
}
