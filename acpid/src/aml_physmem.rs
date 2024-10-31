#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use common::io::{Io, Pio};
use num_traits::PrimInt;
use rustc_hash::FxHashMap;
use std::fmt::LowerHex;
use std::mem::size_of;
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
                common::Prot::RO,
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

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl aml::Handler for AmlPhysMemHandler {
    fn read_u8(&self, address: usize) -> u8 {
        log::trace!("read u8 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u8>(address) {
                return value;
            }
        }
        0
    }
    fn read_u16(&self, address: usize) -> u16 {
        log::trace!("read u16 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u16>(address) {
                return value;
            }
        }
        0
    }
    fn read_u32(&self, address: usize) -> u32 {
        log::trace!("read u32 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u32>(address) {
                return value;
            }
        }
        0
    }
    fn read_u64(&self, address: usize) -> u64 {
        log::trace!("read u64 {:X}", address);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if let Ok(value) = page_cache.read_from_phys::<u64>(address) {
                return value;
            }
        }
        0
    }

    fn write_u8(&mut self, address: usize, value: u8) {
        log::error!("write u8 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u8>(address, value).is_err() {
                log::error!("failed to get page {:#x}", address);
            }
        }
    }
    fn write_u16(&mut self, address: usize, value: u16) {
        log::error!("write u16 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u16>(address, value).is_err() {
                log::error!("failed to get page {:#x}", address);
            }
        }
    }
    fn write_u32(&mut self, address: usize, value: u32) {
        log::error!("write u32 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u32>(address, value).is_err() {
                log::error!("failed to get page {:#x}", address);
            }
        }
    }
    fn write_u64(&mut self, address: usize, value: u64) {
        log::error!("write u64 {:X} = {:X}", address, value);
        if let Ok(mut page_cache) = self.page_cache.lock() {
            if page_cache.write_to_phys::<u64>(address, value).is_err() {
                log::error!("failed to get page {:#x}", address);
            }
        }
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

    fn read_pci_u8(&self, _segment: u16, _bus: u8, _device: u8, _function: u8, _offset: u16) -> u8 {
        log::error!("read pci u8 {:X}, {:X}, {:X}, {:X}, {:X}", _segment, _bus, _device, _function, _offset);

        0
    }
    fn read_pci_u16(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
    ) -> u16 {
        log::error!("read pci u16 {:X}, {:X}, {:X}, {:X}, {:X}", _segment, _bus, _device, _function, _offset);

        0
    }
    fn read_pci_u32(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
    ) -> u32 {
        log::error!("read pci u32 {:X}, {:X}, {:X}, {:X}, {:X}", _segment, _bus, _device, _function, _offset);

        0
    }
    fn write_pci_u8(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u8,
    ) {
        log::error!("write pci u8 {:X}, {:X}, {:X}, {:X}, {:X} = {:X}", _segment, _bus, _device, _function, _offset, _value);
    }
    fn write_pci_u16(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u16,
    ) {
        log::error!("write pci u16 {:X}, {:X}, {:X}, {:X}, {:X} = {:X}", _segment, _bus, _device, _function, _offset, _value);
    }
    fn write_pci_u32(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u32,
    ) {
        log::error!("write pci u32 {:X}, {:X}, {:X}, {:X}, {:X} = {:X}", _segment, _bus, _device, _function, _offset, _value);
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
impl aml::Handler for AmlPhysMemHandler {
    fn read_u8(&self, _address: usize) -> u8 {
        log::error!("read u8 {:X}", _address);
        0
    }
    fn read_u16(&self, _address: usize) -> u16 {
        log::error!("read u16 {:X}", _address);
        0
    }
    fn read_u32(&self, _address: usize) -> u32 {
        log::error!("read u32 {:X}", _address);
        0
    }
    fn read_u64(&self, _address: usize) -> u64 {
        log::error!("read u64 {:X}", _address);
        0
    }

    fn write_u8(&mut self, _address: usize, _value: u8) {
        log::error!("write u8 {:X} = {:X}", _address, _value);
    }
    fn write_u16(&mut self, _address: usize, _value: u16) {
        log::error!("write u16 {:X} = {:X}", _address, _value);
    }
    fn write_u32(&mut self, _address: usize, _value: u32) {
        log::error!("write u32 {:X} = {:X}", _address, _value);
    }
    fn write_u64(&mut self, _address: usize, _value: u64) {
        log::error!("write u64 {:X} = {:X}", _address, _value);
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        log::error!("read io u8 {:X}", port);
        0
    }
    fn read_io_u16(&self, port: u16) -> u16 {
        log::error!("read io u16 {:X}", port);
        0
    }
    fn read_io_u32(&self, port: u16) -> u32 {
        log::error!("read io u32 {:X}", port);
        0
    }

    fn write_io_u8(&self, port: u16, value: u8) {
        log::error!("write io u8 {:X} = {:X}", port, value);
    }
    fn write_io_u16(&self, port: u16, value: u16) {
        log::error!("write io u16 {:X} = {:X}", port, value);
    }
    fn write_io_u32(&self, port: u16, value: u32) {
        log::error!("write io u32 {:X} = {:X}", port, value);
    }

    fn read_pci_u8(&self, _segment: u16, _bus: u8, _device: u8, _function: u8, _offset: u16) -> u8 {
        log::error!("read pci u8 {:X}", _device);

        0
    }
    fn read_pci_u16(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
    ) -> u16 {
        log::error!("read pci  u8 {:X}", _device);

        0
    }
    fn read_pci_u32(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
    ) -> u32 {
        log::error!("read pci u8 {:X}", _device);

        0
    }
    fn write_pci_u8(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u8,
    ) {
        log::error!("write pci u8 {:X}", _device);
    }
    fn write_pci_u16(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u16,
    ) {
        log::error!("write pci u8 {:X}", _device);
    }
    fn write_pci_u32(
        &self,
        _segment: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
        _offset: u16,
        _value: u32,
    ) {
        log::error!("write pci u8 {:X}", _device);
    }
}
