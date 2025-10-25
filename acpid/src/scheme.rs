use acpi::aml::namespace::AmlName;
use amlserde::aml_serde_name::to_aml_format;
use amlserde::AmlSerdeValue;
use core::str;
use parking_lot::RwLockReadGuard;
use redox_scheme::scheme::SchemeSync;
use redox_scheme::{CallerCtx, OpenResult};
use ron::de::SpannedError;
use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;
use syscall::dirent::{DirEntry, DirentBuf, DirentKind};
use syscall::schemev2::NewFdFlags;

use syscall::data::Stat;
use syscall::error::{Error, Result};
use syscall::error::{EBADF, EBADFD, EINVAL, EIO, EISDIR, ENOENT, ENOTDIR};
use syscall::flag::{MODE_DIR, MODE_FILE};
use syscall::flag::{O_ACCMODE, O_DIRECTORY, O_RDONLY, O_STAT, O_SYMLINK};
use syscall::{EOPNOTSUPP, EOVERFLOW, EPERM};

use crate::acpi::{AcpiContext, AmlSymbols, SdtSignature};

pub struct AcpiScheme<'acpi> {
    ctx: &'acpi AcpiContext,
    handles: BTreeMap<usize, Handle<'acpi>>,
    next_fd: usize,
}

struct Handle<'a> {
    kind: HandleKind<'a>,
    stat: bool,
    allowed_to_eval: bool,
}
enum HandleKind<'a> {
    TopLevel,
    Tables,
    Table(SdtSignature),
    Symbols(RwLockReadGuard<'a, AmlSymbols>),
    Symbol { name: String, description: String },
}

impl HandleKind<'_> {
    fn is_dir(&self) -> bool {
        match self {
            Self::TopLevel => true,
            Self::Tables => true,
            Self::Table(_) => false,
            Self::Symbols(_) => true,
            Self::Symbol { .. } => false,
        }
    }
    fn len(&self, acpi_ctx: &AcpiContext) -> Result<usize> {
        Ok(match self {
            // Files
            Self::Table(signature) => acpi_ctx
                .sdt_from_signature(signature)
                .ok_or(Error::new(EBADFD))?
                .length(),
            Self::Symbol { description, .. } => description.len(),
            // Directories
            Self::TopLevel | Self::Symbols(_) | Self::Tables => 0,
        })
    }
}

impl<'acpi> AcpiScheme<'acpi> {
    pub fn new(ctx: &'acpi AcpiContext) -> Self {
        Self {
            ctx,
            handles: BTreeMap::new(),
            next_fd: 0,
        }
    }
}

fn parse_hex_digit(hex: u8) -> Option<u8> {
    let hex = hex.to_ascii_lowercase();

    if hex >= b'a' && hex <= b'f' {
        Some(hex - b'a' + 10)
    } else if hex >= b'0' && hex <= b'9' {
        Some(hex - b'0')
    } else {
        None
    }
}

fn parse_hex_2digit(hex: &[u8]) -> Option<u8> {
    parse_hex_digit(hex[0])
        .and_then(|most_significant| Some((most_significant << 4) | parse_hex_digit(hex[1])?))
}

fn parse_oem_id(hex: [u8; 12]) -> Option<[u8; 6]> {
    Some([
        parse_hex_2digit(&hex[0..2])?,
        parse_hex_2digit(&hex[2..4])?,
        parse_hex_2digit(&hex[4..6])?,
        parse_hex_2digit(&hex[6..8])?,
        parse_hex_2digit(&hex[8..10])?,
        parse_hex_2digit(&hex[10..12])?,
    ])
}
fn parse_oem_table_id(hex: [u8; 16]) -> Option<[u8; 8]> {
    Some([
        parse_hex_2digit(&hex[0..2])?,
        parse_hex_2digit(&hex[2..4])?,
        parse_hex_2digit(&hex[4..6])?,
        parse_hex_2digit(&hex[6..8])?,
        parse_hex_2digit(&hex[8..10])?,
        parse_hex_2digit(&hex[10..12])?,
        parse_hex_2digit(&hex[12..14])?,
        parse_hex_2digit(&hex[14..16])?,
    ])
}

fn parse_table(table: &[u8]) -> Option<SdtSignature> {
    let signature_part = table.get(..4)?;
    let first_hyphen = table.get(4)?;
    let oem_id_part = table.get(5..17)?;
    let second_hyphen = table.get(17)?;
    let oem_table_part = table.get(18..34)?;

    if *first_hyphen != b'-' {
        return None;
    }
    if *second_hyphen != b'-' {
        return None;
    }

    if table.len() > 34 {
        return None;
    }

    Some(SdtSignature {
        signature: <[u8; 4]>::try_from(signature_part)
            .expect("expected 4-byte slice to be convertible into [u8; 4]"),
        oem_id: {
            let hex = <[u8; 12]>::try_from(oem_id_part)
                .expect("expected 12-byte slice to be convertible into [u8; 12]");
            parse_oem_id(hex)?
        },
        oem_table_id: {
            let hex = <[u8; 16]>::try_from(oem_table_part)
                .expect("expected 16-byte slice to be convertible into [u8; 16]");
            parse_oem_table_id(hex)?
        },
    })
}

impl SchemeSync for AcpiScheme<'_> {
    fn open(&mut self, path: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        let path = path.trim_start_matches('/');

        let flag_stat = flags & O_STAT == O_STAT;
        let flag_dir = flags & O_DIRECTORY == O_DIRECTORY;

        // TODO: arrayvec
        let components = {
            let mut v = arrayvec::ArrayVec::<&str, 3>::new();
            let it = path.split('/');
            for component in it.take(3) {
                v.push(component);
            }

            v
        };

        let kind = match &*components {
            [""] => HandleKind::TopLevel,
            ["tables"] => HandleKind::Tables,

            ["tables", table] => {
                let signature = parse_table(table.as_bytes()).ok_or(Error::new(ENOENT))?;
                HandleKind::Table(signature)
            }

            ["symbols"] => {
                if let Ok(aml_symbols) = self.ctx.aml_symbols() {
                    HandleKind::Symbols(aml_symbols)
                } else {
                    return Err(Error::new(EIO));
                }
            }

            ["symbols", symbol] => {
                if let Some(description) = self.ctx.aml_lookup(symbol) {
                    HandleKind::Symbol {
                        name: (*symbol).to_owned(),
                        description,
                    }
                } else {
                    return Err(Error::new(ENOENT));
                }
            }

            _ => return Err(Error::new(ENOENT)),
        };

        if kind.is_dir() && !flag_dir && !flag_stat {
            return Err(Error::new(EISDIR));
        } else if !kind.is_dir() && flag_dir && !flag_stat {
            return Err(Error::new(ENOTDIR));
        }

        let allowed_to_eval = if flags & O_ACCMODE == O_RDONLY || flag_stat {
            false
        } else if ctx.uid == 0 {
            true
        } else {
            return Err(Error::new(EINVAL));
        };

        if flags & O_SYMLINK == O_SYMLINK && !flag_stat {
            return Err(Error::new(EINVAL));
        }

        let fd = self.next_fd;
        self.next_fd += 1;

        self.handles.insert(
            fd,
            Handle {
                stat: flag_stat,
                kind,
                allowed_to_eval,
            },
        );

        Ok(OpenResult::ThisScheme {
            number: fd,
            flags: NewFdFlags::POSITIONED,
        })
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat, _ctx: &CallerCtx) -> Result<()> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        stat.st_size = handle
            .kind
            .len(self.ctx)?
            .try_into()
            .unwrap_or(u64::max_value());

        if handle.kind.is_dir() {
            stat.st_mode = MODE_DIR;
        } else {
            stat.st_mode = MODE_FILE;
        }

        Ok(())
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let offset: usize = offset.try_into().map_err(|_| Error::new(EINVAL))?;

        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        let src_buf = match &handle.kind {
            HandleKind::Table(ref signature) => self
                .ctx
                .sdt_from_signature(signature)
                .ok_or(Error::new(EBADFD))?
                .as_slice(),
            HandleKind::Symbol { description, .. } => description.as_bytes(),
            _ => return Err(Error::new(EINVAL)),
        };

        let offset = std::cmp::min(src_buf.len(), offset);
        let src_buf = &src_buf[offset..];

        let to_copy = std::cmp::min(src_buf.len(), buf.len());

        buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);

        Ok(to_copy)
    }

    fn getdents<'buf>(
        &mut self,
        id: usize,
        mut buf: DirentBuf<&'buf mut [u8]>,
        opaque_offset: u64,
    ) -> Result<DirentBuf<&'buf mut [u8]>> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EOPNOTSUPP))?;

        match &handle.kind {
            HandleKind::TopLevel => {
                const TOPLEVEL_ENTRIES: &[&str] = &["tables", "symbols"];

                for (idx, name) in TOPLEVEL_ENTRIES
                    .iter()
                    .enumerate()
                    .skip(opaque_offset as usize)
                {
                    buf.entry(DirEntry {
                        inode: 0,
                        next_opaque_id: idx as u64 + 1,
                        name,
                        kind: DirentKind::Directory,
                    })?;
                }
            }
            HandleKind::Symbols(aml_symbols) => {
                for (idx, (symbol_name, _value)) in aml_symbols
                    .symbols_cache()
                    .iter()
                    .enumerate()
                    .skip(opaque_offset as usize)
                {
                    buf.entry(DirEntry {
                        inode: 0,
                        next_opaque_id: idx as u64 + 1,
                        name: symbol_name.as_str(),
                        kind: DirentKind::Regular,
                    })?;
                }
            }
            HandleKind::Tables => {
                for (idx, table) in self
                    .ctx
                    .tables()
                    .iter()
                    .enumerate()
                    .skip(opaque_offset as usize)
                {
                    let utf8_or_eio = |bytes| str::from_utf8(bytes).map_err(|_| Error::new(EIO));

                    let mut name = String::new();
                    name.push_str(utf8_or_eio(&table.signature[..])?);
                    name.push('-');
                    for byte in table.oem_id.iter() {
                        std::fmt::write(&mut name, format_args!("{:>02X}", byte)).unwrap();
                    }
                    name.push('-');
                    for byte in table.oem_table_id.iter() {
                        std::fmt::write(&mut name, format_args!("{:>02X}", byte)).unwrap();
                    }

                    buf.entry(DirEntry {
                        inode: 0,
                        next_opaque_id: idx as u64 + 1,
                        name: &name,
                        kind: DirentKind::Regular,
                    })?;
                }
            }
            _ => return Err(Error::new(EIO)),
        }

        Ok(buf)
    }

    fn write(
        &mut self,
        _id: usize,
        _buf: &[u8],
        _offset: u64,
        _fcntl: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        Err(Error::new(EBADF))
    }

    fn call(&mut self, id: usize, payload: &mut [u8], _metadata: &[u64]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;
        if !handle.allowed_to_eval {
            return Err(Error::new(EPERM));
        }

        let Ok(args): Result<Vec<AmlSerdeValue>, SpannedError> = ron::de::from_bytes(payload)
        else {
            return Err(Error::new(EINVAL));
        };

        let HandleKind::Symbol { name, .. } = &handle.kind else {
            return Err(Error::new(EBADF));
        };

        let Ok(aml_name) = AmlName::from_str(&to_aml_format(name)) else {
            log::error!("Failed to convert symbol name: \"{name}\" to aml name!");
            return Err(Error::new(EBADF));
        };

        let Ok(result) = self.ctx.aml_eval(aml_name, args) else {
            return Err(Error::new(EINVAL));
        };

        let Ok(serialized_result) = ron::ser::to_string(&result) else {
            log::error!("Failed to serialize aml result!");
            return Err(Error::new(EINVAL));
        };

        let byte_result = serialized_result.as_bytes();
        let result_len = byte_result.len();

        if result_len > payload.len() {
            return Err(Error::new(EOVERFLOW));
        }

        payload[..result_len].copy_from_slice(byte_result);

        Ok(result_len)
    }
}

impl AcpiScheme<'_> {
    pub fn on_close(&mut self, id: usize) {
        self.handles.remove(&id);
    }
}
