//! Disk scheme replacement when making live disk

#![feature(int_roundings)]

use std::collections::{BTreeMap, HashMap};
use std::fs::File;

use std::os::fd::AsRawFd;

use driver_block::{Disk, DiskScheme};
use driver_block::{ExecutorTrait, TrivialExecutor};
use libredox::call::MmapArgs;
use libredox::flag;

use syscall::error::*;
use syscall::PAGE_SIZE;

use anyhow::{anyhow, Context};

struct LiveDisk {
    original: &'static [u8],
    //TODO: drop overlay blocks if they match the original
    overlay: HashMap<u64, Box<[u8]>>,
}

impl LiveDisk {
    fn new(phys: usize, size: usize) -> anyhow::Result<LiveDisk> {
        let start = phys.div_floor(PAGE_SIZE) * PAGE_SIZE;
        let end = phys
            .checked_add(size)
            .context("phys + size overflow")?
            .next_multiple_of(PAGE_SIZE);
        let size = end - start;

        let original = unsafe {
            let file = File::open("/scheme/memory/physical")?;
            let base = libredox::call::mmap(MmapArgs {
                fd: file.as_raw_fd() as usize,
                addr: core::ptr::null_mut(),
                offset: start as u64,
                length: size,
                prot: flag::PROT_READ,
                flags: flag::MAP_SHARED,
            })
            .map_err(|err| anyhow!("failed to mmap livedisk: {}", err))?;

            std::slice::from_raw_parts_mut(base as *mut u8, size)
        };

        Ok(LiveDisk {
            original,
            overlay: HashMap::new(),
        })
    }
}

impl Disk for LiveDisk {
    fn block_size(&self) -> u32 {
        PAGE_SIZE as u32
    }

    fn size(&self) -> u64 {
        self.original.len() as u64
    }

    async fn read(&mut self, mut block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
        let mut offset = (block as usize) * PAGE_SIZE;
        if offset + buffer.len() > self.original.len() {
            return Err(syscall::Error::new(EINVAL));
        }
        for chunk in buffer.chunks_mut(PAGE_SIZE) {
            match self.overlay.get(&block) {
                Some(overlay) => {
                    chunk.copy_from_slice(&overlay[..chunk.len()]);
                }
                None => {
                    chunk.copy_from_slice(&self.original[offset..offset + chunk.len()]);
                }
            }
            block += 1;
            offset += PAGE_SIZE;
        }
        Ok(buffer.len())
    }

    async fn write(&mut self, mut block: u64, buffer: &[u8]) -> syscall::Result<usize> {
        let mut offset = (block as usize) * PAGE_SIZE;
        if offset + buffer.len() > self.original.len() {
            return Err(syscall::Error::new(EINVAL));
        }
        for chunk in buffer.chunks(PAGE_SIZE) {
            self.overlay.entry(block).or_insert_with(|| {
                let offset = (block as usize) * PAGE_SIZE;
                self.original[offset..offset + PAGE_SIZE]
                    .to_vec()
                    .into_boxed_slice()
            })[..chunk.len()]
                .copy_from_slice(chunk);
            block += 1;
            offset += PAGE_SIZE;
        }
        Ok(buffer.len())
    }
}

fn main() -> anyhow::Result<()> {
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
        // No live disk data, no need to say anything or exit with error
        std::process::exit(0);
    }

    redox_daemon::Daemon::new(move |daemon| {
        let event_queue = event::EventQueue::new().unwrap();

        event::user_data! {
            enum Event {
                Scheme,
            }
        };

        let mut scheme = DiskScheme::new(
            Some(daemon),
            "disk.live".to_owned(),
            BTreeMap::from([(
                0,
                LiveDisk::new(phys, size).unwrap_or_else(|err| {
                    eprintln!("failed to initialize livedisk scheme: {}", err);
                    std::process::exit(1)
                }),
            )]),
            &TrivialExecutor,
        );

        libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

        event_queue
            .subscribe(
                scheme.event_handle().raw(),
                Event::Scheme,
                event::EventFlags::READ,
            )
            .unwrap();

        for event in event_queue {
            match event.unwrap().user_data {
                Event::Scheme => TrivialExecutor.block_on(scheme.tick()).unwrap(),
            }
        }

        std::process::exit(0);
    })
    .map_err(|err| anyhow!("failed to start daemon: {}", err))?;
}
