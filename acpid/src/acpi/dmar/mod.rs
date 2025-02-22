//! DMA Remapping Table -- `DMAR`. This is Intel's implementation of IOMMU functionality, known as
//! VT-d.
//!
//! Too understand what all of these structs mean, refer to the "Intel(R) Virtualization
//! Technology for Directed I/O" specification.

// TODO: Move this code to a separate driver as well?

use std::convert::TryFrom;
use std::ops::Deref;
use std::{fmt, mem};

use common::io::Io as _;

use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

use self::drhd::DrhdPage;
use crate::acpi::{AcpiContext, Sdt, SdtHeader};

pub mod drhd;

#[repr(C, packed)]
pub struct DmarStruct {
    pub sdt_header: SdtHeader,
    pub host_addr_width: u8,
    pub flags: u8,
    pub _rsvd: [u8; 10],
    // This header is followed by N remapping structures.
}
unsafe impl plain::Plain for DmarStruct {}

/// The DMA Remapping Table
#[derive(Debug)]
pub struct Dmar(Sdt);

impl Dmar {
    fn remmapping_structs_area(&self) -> &[u8] {
        &self.0.as_slice()[mem::size_of::<DmarStruct>()..]
    }
}

impl Deref for Dmar {
    type Target = DmarStruct;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(self.0.as_slice())
            .expect("expected Dmar struct to already have checked the length, and alignment issues should be impossible due to #[repr(packed)]")
    }
}

impl Dmar {
    // TODO: Again, perhaps put this code into a different driver, and read the table the regular
    // way via the acpi scheme?
    pub fn init(acpi_ctx: &AcpiContext) {
        let dmar_sdt = match acpi_ctx.take_single_sdt(*b"DMAR") {
            Some(dmar_sdt) => dmar_sdt,
            None => {
                log::warn!("Unable to find `DMAR` ACPI table.");
                return;
            }
        };
        let dmar = match Dmar::new(dmar_sdt) {
            Some(dmar) => dmar,
            None => {
                log::error!("Failed to parse DMAR table, possibly malformed.");
                return;
            }
        };

        log::info!("Found DMAR: {}: {}", dmar.host_addr_width, dmar.flags);
        log::debug!("DMAR: {:?}", dmar);

        for dmar_entry in dmar.iter() {
            log::debug!("DMAR entry: {:?}", dmar_entry);
            match dmar_entry {
                DmarEntry::Drhd(dmar_drhd) => {
                    let drhd = dmar_drhd.map();

                    log::debug!("VER: {:X}", drhd.version.read());
                    log::debug!("CAP: {:X}", drhd.cap.read());
                    log::debug!("EXT_CAP: {:X}", drhd.ext_cap.read());
                    log::debug!("GCMD: {:X}", drhd.gl_cmd.read());
                    log::debug!("GSTS: {:X}", drhd.gl_sts.read());
                    log::debug!("RT: {:X}", drhd.root_table.read());
                }
                _ => (),
            }
        }
    }

    fn new(sdt: Sdt) -> Option<Dmar> {
        assert_eq!(
            sdt.signature, *b"DMAR",
            "signature already checked against `DMAR`"
        );
        if sdt.length() < mem::size_of::<DmarStruct>() {
            log::error!(
                "The DMAR table was too small ({} B < {} B).",
                sdt.length(),
                mem::size_of::<Dmar>()
            );
            return None;
        }
        // No need to check alignment for #[repr(packed)] structs.

        Some(Dmar(sdt))
    }

    pub fn iter(&self) -> DmarIter<'_> {
        DmarIter(DmarRawIter {
            bytes: self.remmapping_structs_area(),
        })
    }
}

/// DMAR DMA Remapping Hardware Unit Definition
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarDrhdHeader {
    pub kind: u16,
    pub length: u16,

    pub flags: u8,
    pub _rsv: u8,
    pub segment: u16,
    pub base: u64,
}
unsafe impl plain::Plain for DmarDrhdHeader {}

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DeviceScopeHeader {
    pub ty: u8,
    pub len: u8,
    pub _rsvd: u16,
    pub enumeration_id: u8,
    pub start_bus_num: u8,
    // The variable-sized path comes after.
}
unsafe impl plain::Plain for DeviceScopeHeader {}

pub struct DeviceScope(Box<[u8]>);

impl DeviceScope {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        // TODO: Check ty.

        let header_bytes = match raw.get(..mem::size_of::<DeviceScopeHeader>()) {
            Some(bytes) => bytes,
            None => return None,
        };
        let header = plain::from_bytes::<DeviceScopeHeader>(header_bytes)
            .expect("length already checked, and alignment 1 (#[repr(packed)] should suffice");

        let len = usize::from(header.len);

        if len > raw.len() {
            log::warn!("Device scope smaller than len field.");
            return None;
        }

        Some(Self(raw.into()))
    }
}

impl fmt::Debug for DeviceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceScope")
            .field("header", &*self as &DeviceScopeHeader)
            .field("path", &self.path())
            .finish()
    }
}

impl Deref for DeviceScope {
    type Target = DeviceScopeHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(&self.0)
            .expect("expected length to be sufficient, and alignment (due to #[repr(packed)]")
    }
}
impl DeviceScope {
    pub fn path(&self) -> &[u8] {
        &self.0[mem::size_of::<DeviceScopeHeader>()..]
    }
}

pub struct DmarDrhd(Box<[u8]>);

impl DmarDrhd {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        if raw.len() < mem::size_of::<DmarDrhdHeader>() {
            return None;
        }

        Some(Self(raw.into()))
    }
    pub fn device_scope_area(&self) -> &[u8] {
        &self.0[mem::size_of::<DmarDrhdHeader>()..]
    }
    pub fn map(&self) -> DrhdPage {
        let base = usize::try_from(self.base).expect("expected u64 to fit within usize");

        DrhdPage::map(base).expect("failed to map DRHD registers")
    }
}
impl Deref for DmarDrhd {
    type Target = DmarDrhdHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes::<DmarDrhdHeader>(&self.0[..mem::size_of::<DmarDrhdHeader>()])
            .expect("length is already checked, and alignment 1 (#[repr(packed)] should suffice")
    }
}
impl fmt::Debug for DmarDrhd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DmarDrhd")
            .field("header", &*self as &DmarDrhd)
            // TODO: print out device scopes
            .finish()
    }
}

/// DMAR Reserved Memory Region Reporting
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarRmrrHeader {
    pub kind: u16,
    pub length: u16,
    pub _rsv: u16,
    pub segment: u16,
    pub base: u64,
    pub limit: u64,
    // The device scopes come after.
}
unsafe impl plain::Plain for DmarRmrrHeader {}

pub struct DmarRmrr(Box<[u8]>);

impl DmarRmrr {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        if raw.len() < mem::size_of::<DmarRmrrHeader>() {
            return None;
        }

        Some(Self(raw.into()))
    }
}
impl Deref for DmarRmrr {
    type Target = DmarRmrrHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(&self.0[..mem::size_of::<DmarRmrrHeader>()])
            .expect("length already checked, and with #[repr(packed)] alignment should be okay")
    }
}
impl fmt::Debug for DmarRmrr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DmarRmrr")
            .field("header", &*self as &DmarRmrrHeader)
            // TODO: print out device scopes
            .finish()
    }
}

/// DMAR Root Port ATS Capability Reporting
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarAtsrHeader {
    kind: u16,
    length: u16,
    flags: u8,
    _rsv: u8,
    segment: u16,
    // The device scopes come after.
}
unsafe impl plain::Plain for DmarAtsrHeader {}

pub struct DmarAtsr(Box<[u8]>);

impl DmarAtsr {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        if raw.len() < mem::size_of::<DmarAtsrHeader>() {
            return None;
        }

        Some(Self(raw.into()))
    }
}
impl Deref for DmarAtsr {
    type Target = DmarAtsrHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(&self.0[..mem::size_of::<DmarAtsrHeader>()])
            .expect("length already checked, and with #[repr(packed)] alignment should be okay")
    }
}
impl fmt::Debug for DmarAtsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DmarAtsr")
            .field("header", &*self as &DmarAtsrHeader)
            // TODO: print out device scopes
            .finish()
    }
}

/// DMAR Remapping Hardware Static Affinity
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarRhsa {
    pub kind: u16,
    pub length: u16,

    pub _rsv: u32,
    pub base: u64,
    pub domain: u32,
}
unsafe impl plain::Plain for DmarRhsa {}
impl DmarRhsa {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        let bytes = raw.get(..mem::size_of::<DmarRhsa>())?;

        let this = plain::from_bytes(bytes)
            .expect("length is already checked, and alignment 1 should suffice (#[repr(packed)])");

        Some(*this)
    }
}

/// DMAR ACPI Name-space Device Declaration
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarAnddHeader {
    pub kind: u16,
    pub length: u16,

    pub _rsv: [u8; 3],
    pub acpi_dev: u8,
    // The device scopes come after.
}
unsafe impl plain::Plain for DmarAnddHeader {}

pub struct DmarAndd(Box<[u8]>);

impl DmarAndd {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        if raw.len() < mem::size_of::<DmarAnddHeader>() {
            return None;
        }

        Some(Self(raw.into()))
    }
}
impl Deref for DmarAndd {
    type Target = DmarAnddHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(&self.0[..mem::size_of::<DmarAnddHeader>()])
            .expect("length already checked, and with #[repr(packed)] alignment should be okay")
    }
}
impl fmt::Debug for DmarAndd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DmarAndd")
            .field("header", &*self as &DmarAnddHeader)
            // TODO: print out device scopes
            .finish()
    }
}

/// DMAR ACPI Name-space Device Declaration
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DmarSatcHeader {
    pub kind: u16,
    pub length: u16,

    pub flags: u8,
    pub _rsvd: u8,
    pub seg_num: u16,
    // The device scopes come after.
}
unsafe impl plain::Plain for DmarSatcHeader {}

pub struct DmarSatc(Box<[u8]>);

impl DmarSatc {
    pub fn try_new(raw: &[u8]) -> Option<Self> {
        if raw.len() < mem::size_of::<DmarSatcHeader>() {
            return None;
        }

        Some(Self(raw.into()))
    }
}

impl Deref for DmarSatc {
    type Target = DmarSatcHeader;

    fn deref(&self) -> &Self::Target {
        plain::from_bytes(&self.0[..mem::size_of::<DmarSatcHeader>()])
            .expect("length already checked, and with #[repr(packed)] alignment should be okay")
    }
}
impl fmt::Debug for DmarSatc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DmarSatc")
            .field("header", &*self as &DmarSatcHeader)
            // TODO: print out device scopes
            .finish()
    }
}

/// The list of different "Remapping Structure Types".
///
/// Refer to section 8.2 in the VTIO spec (as of revision 3.2).
#[derive(Clone, Copy, Debug, FromPrimitive)]
#[repr(u16)]
pub enum EntryType {
    Drhd = 0,
    Rmrr = 1,
    Atsr = 2,
    Rhsa = 3,
    Andd = 4,
    Satc = 5,
}

/// DMAR Entries
#[derive(Debug)]
pub enum DmarEntry {
    Drhd(DmarDrhd),
    Rmrr(DmarRmrr),
    Atsr(DmarAtsr),
    Rhsa(DmarRhsa),
    Andd(DmarAndd),

    // TODO: "SoC Integrated Address Translation Cache Reporting Structure".
    Satc(DmarSatc),

    TooShort(EntryType),
    Unknown(u16),
}

struct DmarRawIter<'sdt> {
    bytes: &'sdt [u8],
}

impl<'sdt> Iterator for DmarRawIter<'sdt> {
    type Item = (u16, &'sdt [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let type_bytes = match self.bytes.get(..2) {
            Some(bytes) => bytes,
            None => {
                if !self.bytes.is_empty() {
                    log::warn!("DMAR table ended between two entries.");
                }
                return None;
            }
        };
        let len_bytes = match self.bytes.get(2..4) {
            Some(bytes) => bytes,
            None => {
                log::warn!("DMAR table ended between two entries.");
                return None;
            }
        };
        let remainder = &self.bytes[4..];

        let type_bytes = <[u8; 2]>::try_from(type_bytes)
            .expect("expected a 2-byte slice to be convertible to [u8; 2]");
        let len_bytes = <[u8; 2]>::try_from(type_bytes)
            .expect("expected a 2-byte slice to be convertible to [u8; 2]");

        let ty = u16::from_ne_bytes(type_bytes);
        let len = u16::from_ne_bytes(len_bytes);

        let len = usize::try_from(len).expect("expected u16 to fit within usize");

        if len > remainder.len() {
            log::warn!("DMAR remapping structure length was smaller than the remaining length of the table.");
            return None;
        }

        let (current, residue) = self.bytes.split_at(len);
        self.bytes = residue;

        Some((ty, current))
    }
}

pub struct DmarIter<'sdt>(DmarRawIter<'sdt>);

impl Iterator for DmarIter<'_> {
    type Item = DmarEntry;
    fn next(&mut self) -> Option<Self::Item> {
        let (raw_type, raw) = self.0.next()?;

        // NOTE: If any of these entries look incorrect, we should simply continue the iterator,
        // and instead print a warning.

        let entry_type = match EntryType::from_u16(raw_type) {
            Some(ty) => ty,
            None => {
                log::warn!(
                    "Encountered invalid entry type {} (length {})",
                    raw_type,
                    raw.len()
                );
                return Some(DmarEntry::Unknown(raw_type));
            }
        };

        let item_opt = match entry_type {
            EntryType::Drhd => DmarDrhd::try_new(raw).map(DmarEntry::Drhd),
            EntryType::Rmrr => DmarRmrr::try_new(raw).map(DmarEntry::Rmrr),
            EntryType::Atsr => DmarAtsr::try_new(raw).map(DmarEntry::Atsr),
            EntryType::Rhsa => DmarRhsa::try_new(raw).map(DmarEntry::Rhsa),
            EntryType::Andd => DmarAndd::try_new(raw).map(DmarEntry::Andd),
            EntryType::Satc => DmarSatc::try_new(raw).map(DmarEntry::Satc),
        };
        let item = item_opt.unwrap_or(DmarEntry::TooShort(entry_type));

        Some(item)
    }
}
