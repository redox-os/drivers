//! Disk scheme replacement when making live disk

#![feature(int_roundings)]

use std::fs::File;

use std::os::fd::AsRawFd;
use std::str;

use libredox::call::MmapArgs;
use libredox::flag;
use redox_scheme::{CallerCtx, OpenResult, RequestKind, SchemeMut, SignalBehavior, Socket, V2};

use syscall::data::Stat;
use syscall::error::*;
use syscall::flag::{MODE_DIR, MODE_FILE};
use syscall::schemev2::NewFdFlags;
use syscall::PAGE_SIZE;

use anyhow::{anyhow, bail, Context};

const LIST: [u8; 2] = *b"0\n";

#[repr(usize)]
enum HandleType {
    TopLevel = 0,
    TheData = 1,
}
impl HandleType {
    fn try_from_raw(raw: usize) -> Option<Self> {
        Some(match raw {
            0 => Self::TopLevel,
            1 => Self::TheData,
            _ => return None,
        })
    }
}

pub struct DiskScheme {
    the_data: &'static mut [u8],
}

impl DiskScheme {
    pub fn new() -> anyhow::Result<DiskScheme> {
        let mut phys = 0;
        let mut size = 0;

        // TODO: handle error
        for line in std::fs::read_to_string("/scheme/sys/env")
            .context("failed to read env")?
            .lines()
        {
            let mut parts = line.splitn(2, '=');
            let name = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");

            if name == "DISK_LIVE_ADDR" {
                phys = usize::from_str_radix(value, 16).unwrap_or(0);
            }

            if name == "DISK_LIVE_SIZE" {
                size = usize::from_str_radix(value, 16).unwrap_or(0);
            }
        }

        if phys == 0 || size == 0 {
            bail!(
                "either livedisk phys ({}) or size ({}) was zero",
                phys,
                size
            );
        }

        let start = phys.div_floor(PAGE_SIZE) * PAGE_SIZE;
        let end = phys
            .checked_add(size)
            .context("phys + size overflow")?
            .next_multiple_of(PAGE_SIZE);
        let size = end - start;

        let the_data = unsafe {
            let file = File::open("/scheme/memory/physical")?;
            let base = libredox::call::mmap(MmapArgs {
                fd: file.as_raw_fd() as usize,
                addr: core::ptr::null_mut(),
                offset: start as u64,
                length: size,
                prot: flag::PROT_READ | flag::PROT_WRITE,
                flags: flag::MAP_SHARED,
            })
            .map_err(|err| anyhow!("failed to mmap livedisk: {}", err))?;

            std::slice::from_raw_parts_mut(base as *mut u8, size)
        };

        Ok(DiskScheme { the_data })
    }
}

impl SchemeMut for DiskScheme {
    fn fsize(&mut self, id: usize) -> Result<u64> {
        Ok(
            match HandleType::try_from_raw(id).ok_or(Error::new(EBADF))? {
                HandleType::TopLevel => LIST.len() as u64,
                HandleType::TheData => self.the_data.len() as u64,
            },
        )
    }

    fn fcntl(&mut self, _id: usize, _cmd: usize, _arg: usize) -> Result<usize> {
        Ok(0)
    }

    fn fsync(&mut self, _id: usize) -> Result<usize> {
        Ok(0)
    }

    fn close(&mut self, _id: usize) -> Result<usize> {
        Ok(0)
    }
    fn xopen(&mut self, path: &str, _flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }

        let path_trimmed = path.trim_matches('/');

        Ok(OpenResult::ThisScheme {
            number: match path_trimmed {
                "" => HandleType::TopLevel as usize,
                "0" => HandleType::TheData as usize,
                _ => return Err(Error::new(ENOENT)),
            },
            flags: NewFdFlags::POSITIONED,
        })
    }
    fn read(&mut self, id: usize, buf: &mut [u8], offset: u64, _flags: u32) -> Result<usize> {
        let data = match HandleType::try_from_raw(id).ok_or(Error::new(EBADF))? {
            HandleType::TheData => &*self.the_data,
            HandleType::TopLevel => &LIST,
        };

        let src = usize::try_from(offset)
            .ok()
            .and_then(|o| data.get(o..))
            .unwrap_or(&[]);
        let byte_count = std::cmp::min(src.len(), buf.len());
        buf[..byte_count].copy_from_slice(&src[..byte_count]);
        Ok(byte_count)
    }
    fn write(&mut self, id: usize, buf: &[u8], offset: u64, _flags: u32) -> Result<usize> {
        let data = match HandleType::try_from_raw(id).ok_or(Error::new(EBADF))? {
            HandleType::TheData => &mut *self.the_data,
            HandleType::TopLevel => return Err(Error::new(EBADF)),
        };

        let dst = usize::try_from(offset)
            .ok()
            .and_then(|o| data.get_mut(o..))
            .unwrap_or(&mut []);
        let byte_count = std::cmp::min(dst.len(), buf.len());
        dst[..byte_count].copy_from_slice(&buf[..byte_count]);
        Ok(byte_count)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let path = match HandleType::try_from_raw(id).ok_or(Error::new(EBADF))? {
            HandleType::TopLevel => "",
            HandleType::TheData => "0",
        };

        let src = format!("disk.live:{}", path).into_bytes();

        let byte_count = std::cmp::min(buf.len(), src.len());
        buf[..byte_count].copy_from_slice(&src[..byte_count]);

        Ok(byte_count)
    }
    fn fstat(&mut self, id: usize, stat_buf: &mut Stat) -> Result<usize> {
        let (len, mode) = match HandleType::try_from_raw(id).ok_or(Error::new(EBADF))? {
            HandleType::TheData => (self.the_data.len(), MODE_FILE | 0o644),
            HandleType::TopLevel => (LIST.len(), MODE_DIR | 0o755),
        };

        *stat_buf = Stat {
            st_mode: mode,
            st_uid: 0,
            st_gid: 0,
            st_size: len.try_into().map_err(|_| Error::new(EOVERFLOW))?,
            ..Stat::default()
        };

        Ok(0)
    }
}
fn main() -> anyhow::Result<()> {
    redox_daemon::Daemon::new(move |daemon| {
        let socket_fd = Socket::<V2>::create("disk.live").expect("failed to open scheme");
        let mut scheme = DiskScheme::new().unwrap_or_else(|err| {
            eprintln!("failed to initialize livedisk scheme: {}", err);
            std::process::exit(1)
        });
        daemon.ready().expect("failed to signal readiness");

        loop {
            let req = match socket_fd
                .next_request(SignalBehavior::Restart)
                .expect("failed to get next request")
            {
                Some(r) => {
                    if let RequestKind::Call(c) = r.kind() {
                        c
                    } else {
                        continue;
                    }
                }
                None => break,
            };
            let resp = req.handle_scheme_mut(&mut scheme);
            socket_fd
                .write_response(resp, SignalBehavior::Restart)
                .expect("failed to write packet");
        }

        std::process::exit(0);
    })
    .map_err(|err| anyhow!("failed to start daemon: {}", err))?;
}
