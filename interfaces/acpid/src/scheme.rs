use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use parking_lot::RwLockReadGuard;

use syscall::data::Stat;
use syscall::error::{EIO, EBADF, EBADFD, EINVAL, EISDIR, ENOENT, ENOTDIR, EOVERFLOW};
use syscall::error::{Error, Result};
use syscall::flag::{O_ACCMODE, O_DIRECTORY, O_RDONLY, O_STAT, O_SYMLINK};
use syscall::flag::{MODE_FILE, MODE_DIR, SEEK_CUR, SEEK_END, SEEK_SET};
use syscall::scheme::SchemeMut;

use crate::acpi::{AcpiContext, SdtSignature, AmlSymbols};

pub struct AcpiScheme<'acpi> {
    ctx: &'acpi AcpiContext,
    handles: BTreeMap<usize, Handle<'acpi>>,
    next_fd: usize,
}

struct Handle<'a> {
    offset: usize,
    kind: HandleKind<'a>,
    stat: bool,
}
enum HandleKind<'a> {
    TopLevel,
    Tables,
    Table(SdtSignature),
    Symbols(RwLockReadGuard<'a, AmlSymbols>),
    Symbol(String),
}

impl HandleKind<'_> {
    fn is_dir(&self) -> bool {
        match self {
            Self::TopLevel => true,
            Self::Tables => true,
            Self::Table(_) => false,
            Self::Symbols(_) => true,
            Self::Symbol(_) => false,
        }
    }
    fn len(&self, acpi_ctx: &AcpiContext) -> Result<usize> {
        Ok(match self {
            Self::TopLevel => TOPLEVEL_CONTENTS.len(),
            Self::Tables => acpi_ctx.tables().len().checked_mul(TABLE_DENTRY_LENGTH).unwrap_or(usize::max_value()),
            Self::Table(signature) => acpi_ctx.sdt_from_signature(signature).ok_or(Error::new(EBADFD))?.length(),
            Self::Symbols(aml_symbols) => aml_symbols.symbols_str().len(),
            Self::Symbol(description) => description.len(),
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

const TOPLEVEL_CONTENTS: &[u8] = b"tables\nsymbols\n";

const TABLE_DENTRY_LENGTH: usize = 35;

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
    parse_hex_digit(hex[0]).and_then(|most_significant| Some((most_significant << 4) | parse_hex_digit(hex[1])?))
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
        signature: <[u8; 4]>::try_from(signature_part).expect("expected 4-byte slice to be convertible into [u8; 4]"),
        oem_id: {
            let hex = <[u8; 12]>::try_from(oem_id_part).expect("expected 12-byte slice to be convertible into [u8; 12]");
            parse_oem_id(hex)?
        },
        oem_table_id: {
            let hex = <[u8; 16]>::try_from(oem_table_part).expect("expected 16-byte slice to be convertible into [u8; 16]");
            parse_oem_table_id(hex)?
        },
    })
}

impl SchemeMut for AcpiScheme<'_> {
    fn open(&mut self, path: &str, flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let path = path.trim_start_matches('/');

        let flag_stat = flags & O_STAT == O_STAT;
        let flag_dir = flags & O_DIRECTORY == O_DIRECTORY;

        // TODO: arrayvec
        let components = path.split('/').collect::<Vec<_>>();

        let kind = match &*components {
            [""] => HandleKind::TopLevel,
            ["tables"] => HandleKind::Tables,

            ["tables", table] => {
                let signature = parse_table(table.as_bytes()).ok_or(Error::new(ENOENT))?;
                HandleKind::Table(signature)
            }

            ["symbols"] => if let Ok(aml_symbols) = self.ctx.aml_symbols() {
                HandleKind::Symbols(aml_symbols)
            } else {
                return Err(Error::new(EIO))
            },

            ["symbols", symbol] => {
                if let Some(description) = self.ctx.aml_lookup(symbol) {
                    HandleKind::Symbol(description)
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

        if flags & O_ACCMODE != O_RDONLY && !flag_stat {
            return Err(Error::new(EINVAL));
        }

        if flags & O_SYMLINK == O_SYMLINK && !flag_stat {
            return Err(Error::new(EINVAL));
        }

        let fd = self.next_fd;
        self.next_fd += 1;

        self.handles.insert(fd, Handle {
            offset: 0,
            stat: flag_stat,
            kind,
        });

        Ok(fd)
    }
    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<usize> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;
        
        stat.st_size = handle.kind.len(self.ctx)?.try_into().unwrap_or(u64::max_value());

        if handle.kind.is_dir() {
            stat.st_mode = MODE_DIR;
        } else {
            stat.st_mode = MODE_FILE;
        }

        Ok(0)
    }
    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        let file_len = handle.kind.len(self.ctx)?;

        let new_offset = match whence {
            SEEK_SET => pos as usize,
            SEEK_CUR => if pos < 0 {
                handle.offset.checked_sub((-pos) as usize).ok_or(Error::new(EINVAL))?
            } else {
                handle.offset.saturating_add(pos as usize)
            },
            SEEK_END => if pos < 0 {
                file_len.checked_sub((-pos) as usize).ok_or(Error::new(EINVAL))?
            } else {
                file_len
            }

            _ => return Err(Error::new(EINVAL)),
        };

        handle.offset = new_offset;
        Ok(new_offset as isize)
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        if handle.stat {
            return Err(Error::new(EBADF));
        }

        let src_buf = match &handle.kind {
            HandleKind::TopLevel => TOPLEVEL_CONTENTS,
            HandleKind::Table(ref signature) => self.ctx.sdt_from_signature(signature).ok_or(Error::new(EBADFD))?.as_slice(),

            HandleKind::Tables => {
                use std::io::prelude::*;

                let tables_to_skip = handle.offset / TABLE_DENTRY_LENGTH;
                let max_tables_to_fill = (buf.len() + TABLE_DENTRY_LENGTH - 1) / TABLE_DENTRY_LENGTH;

                let mut bytes_to_skip = handle.offset % TABLE_DENTRY_LENGTH;

                let mut src_buf = [0_u8; TABLE_DENTRY_LENGTH];
                let mut bytes_written = 0;

                for table in self.ctx.tables().iter().skip(tables_to_skip).take(max_tables_to_fill) {
                    let mut cursor = std::io::Cursor::new(&mut src_buf[..]);
                    cursor.write_all(&table.signature).unwrap();
                    cursor.write_all(&[b'-']).unwrap();
                    // TODO: Treat these IDs as strings?
                    for byte in table.oem_id.iter() {
                        write!(cursor, "{:>02X}", byte).unwrap();
                    }
                    cursor.write_all(&[b'-']).unwrap();
                    for byte in table.oem_table_id.iter() {
                        write!(cursor, "{:>02X}", byte).unwrap();
                    }
                    cursor.write_all(&[b'\n']).unwrap();

                    let src_buf = &src_buf[bytes_to_skip..];
                    let dst_buf = &mut buf[bytes_written..];
                    let to_copy = std::cmp::min(src_buf.len(), dst_buf.len());
                    dst_buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);
                    bytes_written += to_copy;
                    bytes_to_skip = 0;
                }

                handle.offset = handle.offset.checked_add(bytes_written).ok_or(Error::new(EOVERFLOW))?;

                return Ok(bytes_written);
            }

            HandleKind::Symbols(aml_symbols) => {
                let symbols = aml_symbols.symbols_str();
                let offset = std::cmp::min(symbols.len(), handle.offset);
                let src_buf = &symbols.as_bytes()[offset..];

                let to_copy = std::cmp::min(src_buf.len(), buf.len());
                buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);

                handle.offset = handle.offset.checked_add(to_copy).ok_or(Error::new(EOVERFLOW))?;

                return Ok(to_copy);
            }

            HandleKind::Symbol(description) => description.as_bytes(),

        };

        let offset = std::cmp::min(src_buf.len(), handle.offset);
        let src_buf = &src_buf[offset..];

        let to_copy = std::cmp::min(src_buf.len(), buf.len());
        buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);

        handle.offset = handle.offset.checked_add(to_copy).ok_or(Error::new(EOVERFLOW))?;

        Ok(to_copy)
    }
    fn write(&mut self, _id: usize, _buf: &[u8]) -> Result<usize> {
        Err(Error::new(EBADF))
    }
    fn close(&mut self, id: usize) -> Result<usize> {
        if self.handles.remove(&id).is_none() {
            return Err(Error::new(EBADF));
        }

        Ok(0)
    }
}
