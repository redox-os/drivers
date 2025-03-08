//! Disk scheme replacement when making live disk

#![feature(int_roundings)]

use std::collections::BTreeMap;
use std::fs::File;

use std::os::fd::AsRawFd;

use driver_block::{Disk, DiskScheme};
use libredox::call::MmapArgs;
use libredox::flag;

use syscall::error::*;
use syscall::PAGE_SIZE;

use anyhow::{anyhow, bail, Context};

struct LiveDisk {
    the_data: &'static mut [u8],
}

impl LiveDisk {
    fn new() -> anyhow::Result<LiveDisk> {
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

        Ok(LiveDisk { the_data })
    }
}

impl Disk for LiveDisk {
    fn block_size(&self) -> u32 {
        512
    }

    fn size(&self) -> u64 {
        self.the_data.len() as u64
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<Option<usize>> {
        let block = block as usize;
        let block_size = self.block_size() as usize;
        if block * block_size + buffer.len() > self.size() as usize {
            return Err(syscall::Error::new(EOVERFLOW));
        }
        buffer
            .copy_from_slice(&self.the_data[block * block_size..block * block_size + buffer.len()]);
        Ok(Some(block_size))
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<Option<usize>> {
        let block = block as usize;
        let block_size = self.block_size() as usize;
        if block * block_size + buffer.len() > self.size() as usize {
            return Err(syscall::Error::new(EOVERFLOW));
        }
        self.the_data[block * block_size..block * block_size + buffer.len()]
            .copy_from_slice(buffer);
        Ok(Some(block_size))
    }
}

fn main() -> anyhow::Result<()> {
    redox_daemon::Daemon::new(move |daemon| {
        let event_queue = event::EventQueue::new().unwrap();

        event::user_data! {
            enum Event {
                Scheme,
            }
        };

        let mut scheme = DiskScheme::new(
            "disk.live".to_owned(),
            BTreeMap::from([(
                0,
                LiveDisk::new().unwrap_or_else(|err| {
                    eprintln!("failed to initialize livedisk scheme: {}", err);
                    std::process::exit(1)
                }),
            )]),
        );

        libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

        event_queue
            .subscribe(
                scheme.event_handle().raw(),
                Event::Scheme,
                event::EventFlags::READ,
            )
            .unwrap();

        daemon.ready().expect("failed to signal readiness");

        for event in event_queue {
            match event.unwrap().user_data {
                Event::Scheme => scheme.tick().unwrap(),
            }
        }

        std::process::exit(0);
    })
    .map_err(|err| anyhow!("failed to start daemon: {}", err))?;
}
