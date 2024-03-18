//! Disk scheme replacement when making live disk

#![feature(int_roundings)]

use std::fs::File;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::str;

use libredox::call::MmapArgs;
use libredox::flag;
use slab::Slab;
use syscall::data::Stat;
use syscall::{error::*, MapFlags, SchemeMut, Packet};
use syscall::flag::{MODE_DIR, MODE_FILE};
use syscall::scheme::calc_seek_offset_usize;
use syscall::PAGE_SIZE;

use anyhow::{anyhow, Context, bail};

const LIST: [u8; 2] = *b"0\n";

struct Handle {
    ty: HandleType,
    seek: usize,
}
enum HandleType {
    TopLevel,
    TheData,
}

pub struct DiskScheme {
    the_data: &'static mut [u8],
    handles: Slab<Handle>,
}

impl DiskScheme {
    pub fn new() -> anyhow::Result<DiskScheme> {
        let mut phys = 0;
        let mut size = 0;

        // TODO: handle error
        for line in std::fs::read_to_string("sys:env").context("failed to read env")?.lines() {
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
            bail!("either livedisk phys ({}) or size ({}) was zero", phys, size);
        }

        let start = phys.div_floor(PAGE_SIZE) * PAGE_SIZE;
        let end = phys.checked_add(size).context("phys + size overflow")?.next_multiple_of(PAGE_SIZE);
        let size = end - start;

        let the_data = unsafe {
            let file = File::open("memory:physical")?;
            let base = libredox::call::mmap(MmapArgs {
                fd: file.as_raw_fd() as usize,
                addr: core::ptr::null_mut(),
                offset: start as u64,
                length: size,
                prot: flag::PROT_READ | flag::PROT_WRITE,
                flags: flag::MAP_SHARED,
            }).map_err(|err| anyhow!("failed to mmap livedisk: {}", err))?;

            std::slice::from_raw_parts_mut(base as *mut u8, size)
        };

        Ok(DiskScheme {
            the_data,
            handles: Slab::with_capacity(32),
        })
    }
}

impl SchemeMut for DiskScheme {
    fn seek(&mut self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        let handle = self.handles.get_mut(id).ok_or(Error::new(EBADF))?;
        let len = match handle.ty {
            HandleType::TopLevel => LIST.len(),
            HandleType::TheData => self.the_data.len(),
        };
        let new_offset = calc_seek_offset_usize(handle.seek, pos, whence, len)?;
        handle.seek = new_offset as usize;
        Ok(new_offset)
    }

    fn fcntl(&mut self, id: usize, _cmd: usize, _arg: usize) -> Result<usize> {
        let _handle = self.handles.get(id).ok_or(Error::new(EBADF))?;

        Ok(0)
    }

    fn fsync(&mut self, id: usize) -> Result<usize> {
        let _handle = self.handles.get(id).ok_or(Error::new(EBADF))?;

        Ok(0)
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        let _ = self.handles.remove(id);

        Ok(0)
    }
    fn open(&mut self, path: &str, _flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid != 0 {
            return Err(Error::new(EACCES));
        }

        let path_trimmed = path.trim_matches('/');

        let handle = match path_trimmed {
            "" => {
                Handle {
                    //mode: MODE_DIR | 0o755,
                    seek: 0,
                    ty: HandleType::TopLevel,
                }
            },
            "0" => {
                Handle {
                    //mode: MODE_FILE | 0o644,
                    seek: 0,
                    ty: HandleType::TheData,
                }
            }
            _ => return Err(Error::new(ENOENT)),
        };

        Ok(self.handles.insert(handle))
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let handle = self.handles.get_mut(id).ok_or(Error::new(EBADF))?;

        let data = match handle.ty {
            HandleType::TheData => &*self.the_data,
            HandleType::TopLevel => &LIST,
        };

        let src = data.get(handle.seek..).unwrap_or(&[]);
        let byte_count = std::cmp::min(src.len(), buf.len());
        buf[..byte_count].copy_from_slice(&src[..byte_count]);
        handle.seek += byte_count;

        Ok(byte_count)
    }

    fn write(&mut self, id: usize, buffer: &[u8]) -> Result<usize> {
        let handle = self.handles.get_mut(id).ok_or(Error::new(EBADF))?;

        match handle.ty {
            HandleType::TheData => {
                let dst = self.the_data.get_mut(handle.seek..).unwrap_or(&mut []);
                let byte_count = std::cmp::min(dst.len(), buffer.len());
                dst[..byte_count].copy_from_slice(&buffer[..byte_count]);
                handle.seek += byte_count;

                Ok(byte_count)
            },
            HandleType::TopLevel => Err(Error::new(EBADF)),
        }
    }
    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let path = match self.handles.get(id).ok_or(Error::new(EBADF))?.ty {
            HandleType::TopLevel => "",
            HandleType::TheData => "0",
        };

        let src = format!("disk.live:{}", path).into_bytes();

        let byte_count = std::cmp::min(buf.len(), src.len());
        buf[..byte_count].copy_from_slice(&src[..byte_count]);

        Ok(byte_count)
    }
    fn fstat(&mut self, id: usize, stat_buf: &mut Stat) -> Result<usize> {
        let handle = self.handles.get(id).ok_or(Error::new(EBADF))?;

        let (len, mode) = match handle.ty {
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
        let mut socket = File::create(":disk.live").expect("failed to open scheme");
        let mut scheme = DiskScheme::new().unwrap_or_else(|err| {
            eprintln!("failed to initialize livedisk scheme: {}", err);
            std::process::exit(1)
        });
        daemon.ready().expect("failed to signal readiness");

        let mut packet = Packet::default();

        loop {
            socket.read_exact(&mut packet).expect("failed to read packet");
            scheme.handle(&mut packet);
            socket.write_all(&packet).expect("failed to write packet");
        }

    }).map_err(|err| anyhow!("failed to start daemon: {}", err))?;
}
