use acpi::aml::object::{Object, WrappedObject};
use rustc_hash::FxHashMap;
use std::convert::{TryFrom, TryInto};
use std::error::Error;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::{fmt, mem};
use syscall::PAGE_SIZE;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use common::io::{Io, Pio};

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use thiserror::Error;

use acpi::{
    aml::{namespace::AmlName, AmlError, Interpreter},
    platform::AcpiPlatform,
    AcpiTables,
};
use amlserde::aml_serde_name::aml_to_symbol;
use amlserde::{AmlSerde, AmlSerdeValue};

#[cfg(target_arch = "x86_64")]
pub mod dmar;
use crate::aml_physmem::{AmlPageCache, AmlPhysMemHandler};

/// The raw SDT header struct, as defined by the ACPI specification.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}
unsafe impl plain::Plain for SdtHeader {}

impl SdtHeader {
    pub fn signature(&self) -> SdtSignature {
        SdtSignature {
            signature: self.signature,
            oem_id: self.oem_id,
            oem_table_id: self.oem_table_id,
        }
    }
    pub fn length(&self) -> usize {
        self.length
            .try_into()
            .expect("expected usize to be at least 32 bits")
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdtSignature {
    pub signature: [u8; 4],
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
}

impl fmt::Display for SdtSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            String::from_utf8_lossy(&self.signature),
            String::from_utf8_lossy(&self.oem_id),
            String::from_utf8_lossy(&self.oem_table_id)
        )
    }
}

#[derive(Debug, Error)]
pub enum TablePhysLoadError {
    // TODO: Make syscall::Error implement std::error::Error, when enabling a Cargo feature.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid SDT: {0}")]
    Validity(#[from] InvalidSdtError),
}
#[derive(Debug, Error)]
pub enum InvalidSdtError {
    #[error("invalid size")]
    InvalidSize,

    #[error("invalid checksum")]
    BadChecksum,
}

struct PhysmapGuard {
    virt: *const u8,
    size: usize,
}
impl PhysmapGuard {
    fn map(page: usize, page_count: usize) -> std::io::Result<Self> {
        let size = page_count * PAGE_SIZE;
        let virt = unsafe {
            common::physmap(page, size, common::Prot::RO, common::MemoryType::default())
                .map_err(|error| std::io::Error::from_raw_os_error(error.errno()))?
        };

        Ok(Self {
            virt: virt as *const u8,
            size,
        })
    }
}
impl Deref for PhysmapGuard {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.virt as *const u8, self.size) }
    }
}
impl Drop for PhysmapGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.virt as *mut (), self.size);
        }
    }
}

#[derive(Clone)]
pub struct Sdt(Arc<[u8]>);

impl Sdt {
    pub fn new(slice: Arc<[u8]>) -> Result<Self, InvalidSdtError> {
        let header = match plain::from_bytes::<SdtHeader>(&slice) {
            Ok(header) => header,
            Err(plain::Error::TooShort) => return Err(InvalidSdtError::InvalidSize),
            Err(plain::Error::BadAlignment) => panic!(
                "plain::from_bytes failed due to alignment, but SdtHeader is #[repr(packed)]!"
            ),
        };

        if header.length() != slice.len() {
            return Err(InvalidSdtError::InvalidSize);
        }

        let checksum = slice
            .iter()
            .copied()
            .fold(0_u8, |current_sum, item| current_sum.wrapping_add(item));

        if checksum != 0 {
            return Err(InvalidSdtError::BadChecksum);
        }

        Ok(Self(slice))
    }
    pub fn load_from_physical(physaddr: usize) -> Result<Self, TablePhysLoadError> {
        let physaddr_start_page = physaddr / PAGE_SIZE * PAGE_SIZE;
        let physaddr_page_offset = physaddr % PAGE_SIZE;

        // Begin by reading and validating the header first. The SDT header is always 36 bytes
        // long, and can thus span either one or two page table frames.
        let needs_extra_page = (PAGE_SIZE - physaddr_page_offset)
            .checked_sub(mem::size_of::<SdtHeader>())
            .is_none();
        let page_table_count = 1 + if needs_extra_page { 1 } else { 0 };

        let pages = PhysmapGuard::map(physaddr_start_page, page_table_count)?;
        assert!(pages.len() >= mem::size_of::<SdtHeader>());
        let sdt_mem = &pages[physaddr_page_offset..];

        let sdt = plain::from_bytes::<SdtHeader>(&sdt_mem[..mem::size_of::<SdtHeader>()])
            .expect("either alignment is wrong, or the length is too short, both of which are already checked for");

        let total_length = sdt.length();
        let base_length = std::cmp::min(total_length, sdt_mem.len());
        let extended_length = total_length - base_length;

        let mut loaded = sdt_mem[..base_length].to_owned();
        loaded.reserve(extended_length);

        const SIMULTANEOUS_PAGE_COUNT: usize = 4;

        let mut left = extended_length;
        let mut offset = physaddr_start_page + page_table_count * PAGE_SIZE;

        let length_per_iteration = PAGE_SIZE * SIMULTANEOUS_PAGE_COUNT;

        while left > 0 {
            let to_copy = std::cmp::min(left, length_per_iteration);
            let additional_pages = PhysmapGuard::map(offset, to_copy.div_ceil(PAGE_SIZE))?;

            loaded.extend(&additional_pages[..to_copy]);

            left -= to_copy;
            offset += to_copy;
        }
        assert_eq!(left, 0);

        Self::new(loaded.into()).map_err(Into::into)
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for Sdt {
    type Target = SdtHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes::<SdtHeader>(&self.0)
            .expect("expected already validated Sdt to be able to get its header")
    }
}

impl Sdt {
    pub fn data(&self) -> &[u8] {
        &self.0[mem::size_of::<SdtHeader>()..]
    }
}

impl fmt::Debug for Sdt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sdt")
            .field("header", &*self as &SdtHeader)
            .field("extra_len", &self.data().len())
            .finish()
    }
}

pub struct Dsdt(Sdt);
pub struct Ssdt(Sdt);

// Current AML implementation builds the aml_context.namespace at startup,
// but the cache for symbols is lazy-loaded when someone
// reads from the acpi:/symbols scheme.
// If you dynamically add an SDT, you can add to the namespace, but you
// must empty the cache so it is rebuilt.
// If you modify an SDT, you must discard the aml_context and rebuild it.
pub struct AmlSymbols {
    aml_context: Option<Interpreter<AmlPhysMemHandler>>,
    // k = name, v = description
    symbol_cache: FxHashMap<String, String>,
    page_cache: Arc<Mutex<AmlPageCache>>,
}

impl AmlSymbols {
    pub fn new() -> Self {
        Self {
            aml_context: None,
            symbol_cache: FxHashMap::default(),
            page_cache: Arc::new(Mutex::new(AmlPageCache::default())),
        }
    }

    pub fn init(&mut self) -> Result<(), Box<dyn Error>> {
        if self.aml_context.is_some() {
            return Err("AML interpreter already initialized".into());
        }
        let format_err = |err| format!("{:?}", err);
        let handler = AmlPhysMemHandler::new(Arc::clone(&self.page_cache));
        //TODO: use these parsed tables for the rest of acpid
        let rsdp_address = usize::from_str_radix(&std::env::var("RSDP_ADDR")?, 16)?;
        let tables = unsafe { AcpiTables::from_rsdp(handler.clone(), rsdp_address).map_err(format_err)? };
        let platform = AcpiPlatform::new(tables, handler).map_err(format_err)?;
        let interpreter = Interpreter::new_from_platform(&platform).map_err(format_err)?;
        self.aml_context = Some(interpreter);
        Ok(())
    }

    pub fn aml_context_mut(&mut self) -> &mut Interpreter<AmlPhysMemHandler> {
        // PCID must be running by this time!
        if self.aml_context.is_none() {
            match self.init() {
                Ok(()) => (),
                Err(err) => {
                    log::error!("failed to initialize AML context: {}", err);
                }
            }
        }
        self.aml_context.as_mut().expect("AML context not initialized")
    }

    pub fn symbols_cache(&self) -> &FxHashMap<String, String> {
        &self.symbol_cache
    }

    pub fn lookup(&self, symbol: &str) -> Option<String> {
        if let Some(description) = self.symbol_cache.get(symbol) {
            log::trace!("Found symbol in cache, {}, {}", symbol, description);
            return Some(description.to_owned());
        }
        None
    }

    pub fn build_cache(&mut self) {
        let aml_context = self.aml_context_mut();

        let mut symbol_list: Vec<(AmlName, String)> = Vec::with_capacity(5000);

        if aml_context
            .namespace
            .lock()
            .traverse(|level_aml_name, level| {
                for (child_seg, handle) in level.values.iter() {
                    if let Ok(aml_name) =
                        AmlName::from_name_seg(child_seg.to_owned()).resolve(level_aml_name)
                    {
                        let name = aml_to_symbol(&aml_name);
                        symbol_list.push((aml_name, name));
                    } else {
                        log::error!(
                            "AmlName resolve failed, {:?}:{:?}",
                            level_aml_name,
                            child_seg
                        );
                    }
                }
                Ok(true)
            })
            .is_err()
        {
            log::error!("Namespace traverse failed");
            return;
        }

        let mut symbol_cache: FxHashMap<String, String> = FxHashMap::default();

        for (aml_name, name) in &symbol_list {
            // create an empty entry, in case something goes wrong with serialization
            symbol_cache.insert(name.to_owned(), "".to_owned());
            if let Some(ser_value) = AmlSerde::from_aml(aml_context, aml_name) {
                if let Ok(ser_string) = ron::ser::to_string_pretty(&ser_value, Default::default()) {
                    // replace the empty entry
                    symbol_cache.insert(name.to_owned(), ser_string);
                }
            }
        }

        // Cache the new list
        log::trace!("Updating symbols list");

        self.symbol_cache = symbol_cache;
    }
}

#[derive(Debug, Error)]
pub enum AmlEvalError {
    #[error("AML error")]
    AmlError(AmlError),
    #[error("Failed to serialize argument")]
    SerializationError,
    #[error("Failed to deserialize")]
    DeserializationError,
}
impl From<AmlError> for AmlEvalError {
    fn from(value: AmlError) -> Self {
        AmlEvalError::AmlError(value)
    }
}

pub struct AcpiContext {
    tables: Vec<Sdt>,
    dsdt: Option<Dsdt>,
    fadt: Option<Fadt>,

    aml_symbols: RwLock<AmlSymbols>,

    // TODO: The kernel ACPI code seemed to use load_table quite ubiquitously, however ACPI 5.1
    // states that DDBHandles can only be obtained when loading XSDT-pointed tables. So, we'll
    // generate an index only for those.
    sdt_order: RwLock<Vec<Option<SdtSignature>>>,

    pub next_ctx: RwLock<u64>,
}

impl AcpiContext {
    pub fn aml_eval(
        &self,
        symbol: AmlName,
        args: Vec<AmlSerdeValue>,
    ) -> Result<AmlSerdeValue, AmlEvalError> {
        let mut symbols = self.aml_symbols.write();
        let interpreter = symbols.aml_context_mut();
        interpreter.acquire_global_lock(16)?;

        let args = args
            .into_iter()
            .map(|aml_serde_value| {
                aml_serde_value
                    .to_aml_object()
                    .map(Object::wrap)
                    .ok_or(AmlEvalError::DeserializationError)
            })
            .collect::<Result<Vec<WrappedObject>, AmlEvalError>>()?;

        let result = interpreter.evaluate(symbol, args);
        interpreter
            .release_global_lock()
            .expect("Failed to release GIL!"); //TODO: check if this should panic

        result
            .map_err(AmlEvalError::from)
            .map(|object| {
                AmlSerdeValue::from_aml_value(object.deref())
                    .ok_or(AmlEvalError::SerializationError)
            })
            .flatten()
    }

    pub fn init(rxsdt_physaddrs: impl Iterator<Item = u64>) -> Self {
        let tables = rxsdt_physaddrs
            .map(|physaddr| {
                let physaddr: usize = physaddr
                    .try_into()
                    .expect("expected ACPI addresses to be compatible with the current word size");

                log::trace!("TABLE AT {:#>08X}", physaddr);

                Sdt::load_from_physical(physaddr).expect("failed to load physical SDT")
            })
            .collect::<Vec<Sdt>>();

        let mut this = Self {
            tables,
            dsdt: None,
            fadt: None,

            // Temporary values
            aml_symbols: RwLock::new(AmlSymbols::new()),

            next_ctx: RwLock::new(0),

            sdt_order: RwLock::new(Vec::new()),
        };

        for table in &this.tables {
            this.new_index(&table.signature());
        }

        Fadt::init(&mut this);
        //TODO (hangs on real hardware): Dmar::init(&this);

        this
    }

    pub fn dsdt(&self) -> Option<&Dsdt> {
        self.dsdt.as_ref()
    }
    pub fn ssdts(&self) -> impl Iterator<Item = Ssdt> + '_ {
        self.find_multiple_sdts(*b"SSDT")
            .map(|sdt| Ssdt(sdt.clone()))
    }
    fn find_single_sdt_pos(&self, signature: [u8; 4]) -> Option<usize> {
        let count = self
            .tables
            .iter()
            .filter(|sdt| sdt.signature == signature)
            .count();

        if count > 1 {
            log::warn!(
                "Expected only a single SDT of signature `{}` ({:?}), but there were {}",
                String::from_utf8_lossy(&signature),
                signature,
                count
            );
        }

        self.tables
            .iter()
            .position(|sdt| sdt.signature == signature)
    }
    pub fn find_multiple_sdts<'a>(&'a self, signature: [u8; 4]) -> impl Iterator<Item = &'a Sdt> {
        self.tables
            .iter()
            .filter(move |sdt| sdt.signature == signature)
    }
    pub fn take_single_sdt(&self, signature: [u8; 4]) -> Option<Sdt> {
        self.find_single_sdt_pos(signature)
            .map(|pos| self.tables[pos].clone())
    }
    pub fn fadt(&self) -> Option<&Fadt> {
        self.fadt.as_ref()
    }
    pub fn sdt_from_signature(&self, signature: &SdtSignature) -> Option<&Sdt> {
        self.tables.iter().find(|sdt| {
            sdt.signature == signature.signature
                && sdt.oem_id == signature.oem_id
                && sdt.oem_table_id == signature.oem_table_id
        })
    }
    pub fn get_signature_from_index(&self, index: usize) -> Option<SdtSignature> {
        self.sdt_order.read().get(index).copied().flatten()
    }
    pub fn get_index_from_signature(&self, signature: &SdtSignature) -> Option<usize> {
        self.sdt_order
            .read()
            .iter()
            .rposition(|sig| sig.map_or(false, |sig| &sig == signature))
    }
    pub fn tables(&self) -> &[Sdt] {
        &self.tables
    }
    pub fn new_index(&self, signature: &SdtSignature) {
        self.sdt_order.write().push(Some(*signature));
    }

    pub fn aml_lookup(&self, symbol: &str) -> Option<String> {
        if let Ok(aml_symbols) = self.aml_symbols() {
            aml_symbols.lookup(symbol)
        } else {
            None
        }
    }

    pub fn aml_symbols(&self) -> Result<RwLockReadGuard<'_, AmlSymbols>, AmlError> {
        // return the cached value if it exists
        let symbols = self.aml_symbols.read();
        if !symbols.symbols_cache().is_empty() {
            return Ok(symbols);
        }
        // free the read lock
        drop(symbols);

        // List has not been initialized, we have to build it
        log::trace!("Creating symbols list");

        let mut aml_symbols = self.aml_symbols.write();

        aml_symbols.build_cache();

        // return the cached value
        Ok(RwLockWriteGuard::downgrade(aml_symbols))
    }

    /// Discard any cached symbols list. To be called if the AML namespace changes.
    pub fn aml_symbols_reset(&self) {
        let mut aml_symbols = self.aml_symbols.write();
        aml_symbols.symbol_cache = FxHashMap::default();
    }

    /// Set Power State
    /// See https://uefi.org/sites/default/files/resources/ACPI_6_1.pdf
    /// - search for PM1a
    /// See https://forum.osdev.org/viewtopic.php?t=16990 for practical details
    pub fn set_global_s_state(&self, state: u8) {
        if state != 5 {
            return;
        }
        let fadt = match self.fadt() {
            Some(fadt) => fadt,
            None => {
                log::error!("Cannot set global S-state due to missing FADT.");
                return;
            }
        };

        let port = fadt.pm1a_control_block as u16;
        let mut val = 1 << 13;

        let aml_symbols = self.aml_symbols.read();

        let s5_aml_name = match acpi::aml::namespace::AmlName::from_str("\\_S5") {
            Ok(aml_name) => aml_name,
            Err(error) => {
                log::error!("Could not build AmlName for \\_S5, {:?}", error);
                return;
            }
        };

        let s5 = match &aml_symbols.aml_context {
            Some(aml_context) => match aml_context.namespace.lock().get(s5_aml_name) {
                Ok(s5) => s5,
                Err(error) => {
                    log::error!("Cannot set S-state, missing \\_S5, {:?}", error);
                    return;
                }
            },
            None => {
                log::error!("Cannot set S-state, AML context not initialized");
                return;
            }
        };

        let package = match s5.deref() {
            acpi::aml::object::Object::Package(package) => package,
            _ => {
                log::error!("Cannot set S-state, \\_S5 is not a package");
                return;
            }
        };

        let slp_typa = match package[0].deref() {
            acpi::aml::object::Object::Integer(i) => i.to_owned(),
            _ => {
                log::error!("typa is not an Integer");
                return;
            }
        };
        let slp_typb = match package[1].deref() {
            acpi::aml::object::Object::Integer(i) => i.to_owned(),
            _ => {
                log::error!("typb is not an Integer");
                return;
            }
        };

        log::trace!("Shutdown SLP_TYPa {:X}, SLP_TYPb {:X}", slp_typa, slp_typb);
        val |= slp_typa as u16;

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            log::warn!("Shutdown with ACPI outw(0x{:X}, 0x{:X})", port, val);
            Pio::<u16>::new(port).write(val);
        }

        // TODO: Handle SLP_TYPb

        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            log::error!(
                "Cannot shutdown with ACPI outw(0x{:X}, 0x{:X}) on this architecture",
                port,
                val
            );
        }

        loop {
            core::hint::spin_loop();
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct FadtStruct {
    pub header: SdtHeader,
    pub firmware_ctrl: u32,
    pub dsdt: u32,

    // field used in ACPI 1.0; no longer in use, for compatibility only
    reserved: u8,

    pub preferred_power_managament: u8,
    pub sci_interrupt: u16,
    pub smi_command_port: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub s4_bios_req: u8,
    pub pstate_control: u8,
    pub pm1a_event_block: u32,
    pub pm1b_event_block: u32,
    pub pm1a_control_block: u32,
    pub pm1b_control_block: u32,
    pub pm2_control_block: u32,
    pub pm_timer_block: u32,
    pub gpe0_block: u32,
    pub gpe1_block: u32,
    pub pm1_event_length: u8,
    pub pm1_control_length: u8,
    pub pm2_control_length: u8,
    pub pm_timer_length: u8,
    pub gpe0_ength: u8,
    pub gpe1_length: u8,
    pub gpe1_base: u8,
    pub c_state_control: u8,
    pub worst_c2_latency: u16,
    pub worst_c3_latency: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alarm: u8,
    pub month_alarm: u8,
    pub century: u8,

    // reserved in ACPI 1.0; used since ACPI 2.0+
    pub boot_architecture_flags: u16,

    reserved2: u8,
    pub flags: u32,
}
unsafe impl plain::Plain for FadtStruct {}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GenericAddressStructure {
    address_space: u8,
    bit_width: u8,
    bit_offset: u8,
    access_size: u8,
    address: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct FadtAcpi2Struct {
    // 12 byte structure; see below for details
    pub reset_reg: GenericAddressStructure,

    pub reset_value: u8,
    reserved3: [u8; 3],

    // 64bit pointers - Available on ACPI 2.0+
    pub x_firmware_control: u64,
    pub x_dsdt: u64,

    pub x_pm1a_event_block: GenericAddressStructure,
    pub x_pm1b_event_block: GenericAddressStructure,
    pub x_pm1a_control_block: GenericAddressStructure,
    pub x_pm1b_control_block: GenericAddressStructure,
    pub x_pm2_control_block: GenericAddressStructure,
    pub x_pm_timer_block: GenericAddressStructure,
    pub x_gpe0_block: GenericAddressStructure,
    pub x_gpe1_block: GenericAddressStructure,
}
unsafe impl plain::Plain for FadtAcpi2Struct {}

#[derive(Clone)]
pub struct Fadt(Sdt);

impl Fadt {
    pub fn acpi_2_struct(&self) -> Option<&FadtAcpi2Struct> {
        let bytes = &self.0 .0[mem::size_of::<FadtStruct>()..];

        match plain::from_bytes::<FadtAcpi2Struct>(bytes) {
            Ok(fadt2) => Some(fadt2),
            Err(plain::Error::TooShort) => None,
            Err(plain::Error::BadAlignment) => unreachable!(
                "plain::from_bytes reported bad alignment, but FadtAcpi2Struct is #[repr(packed)]"
            ),
        }
    }
}

impl Deref for Fadt {
    type Target = FadtStruct;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes::<FadtStruct>(&self.0 .0)
            .expect("expected FADT struct to already be validated in Deref impl")
    }
}

impl Fadt {
    pub fn new(sdt: Sdt) -> Option<Fadt> {
        if sdt.signature != *b"FACP" || sdt.length() < mem::size_of::<Fadt>() {
            return None;
        }
        Some(Fadt(sdt))
    }

    pub fn init(context: &mut AcpiContext) {
        let fadt_sdt = context
            .take_single_sdt(*b"FACP")
            .expect("expected ACPI to always have a FADT");

        let fadt = match Fadt::new(fadt_sdt) {
            Some(fadt) => fadt,
            None => {
                log::error!("Failed to find FADT");
                return;
            }
        };

        let dsdt_ptr = match fadt.acpi_2_struct() {
            Some(fadt2) => usize::try_from(fadt2.x_dsdt).unwrap_or_else(|_| {
                usize::try_from(fadt.dsdt).expect("expected any given u32 to fit within usize")
            }),
            None => usize::try_from(fadt.dsdt).expect("expected any given u32 to fit within usize"),
        };

        log::debug!("FACP at {:X}", { dsdt_ptr });

        let dsdt_sdt = match Sdt::load_from_physical(fadt.dsdt as usize) {
            Ok(dsdt) => dsdt,
            Err(error) => {
                log::error!("Failed to load DSDT: {}", error);
                return;
            }
        };

        context.fadt = Some(fadt.clone());
        context.dsdt = Some(Dsdt(dsdt_sdt.clone()));

        context.tables.push(dsdt_sdt);
    }
}

pub enum PossibleAmlTables {
    Dsdt(Dsdt),
    Ssdt(Ssdt),
}
impl PossibleAmlTables {
    pub fn try_new(inner: Sdt) -> Option<Self> {
        match &inner.signature {
            b"DSDT" => Some(Self::Dsdt(Dsdt(inner))),
            b"SSDT" => Some(Self::Ssdt(Ssdt(inner))),
            _ => None,
        }
    }
}
impl AmlContainingTable for PossibleAmlTables {
    fn aml(&self) -> &[u8] {
        match self {
            Self::Dsdt(dsdt) => dsdt.aml(),
            Self::Ssdt(ssdt) => ssdt.aml(),
        }
    }
    fn header(&self) -> &SdtHeader {
        match self {
            Self::Dsdt(dsdt) => dsdt.header(),
            Self::Ssdt(ssdt) => ssdt.header(),
        }
    }
}

pub trait AmlContainingTable {
    fn aml(&self) -> &[u8];
    fn header(&self) -> &SdtHeader;
}

impl<T> AmlContainingTable for &T
where
    T: AmlContainingTable,
{
    fn aml(&self) -> &[u8] {
        T::aml(*self)
    }
    fn header(&self) -> &SdtHeader {
        T::header(*self)
    }
}

impl AmlContainingTable for Dsdt {
    fn aml(&self) -> &[u8] {
        self.0.data()
    }
    fn header(&self) -> &SdtHeader {
        &*self.0
    }
}
impl AmlContainingTable for Ssdt {
    fn aml(&self) -> &[u8] {
        self.0.data()
    }
    fn header(&self) -> &SdtHeader {
        &*self.0
    }
}
