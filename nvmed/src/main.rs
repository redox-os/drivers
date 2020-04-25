use std::{env, slice, usize};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::ptr::NonNull;
use std::os::unix::io::{RawFd, FromRawFd};
use std::sync::{Arc, Mutex};

use pcid_interface::{PciFeature, PciFeatureInfo, PciFunction, PcidServerHandle, PciBar};
use syscall::{EVENT_READ, PHYSMAP_NO_CACHE, PHYSMAP_WRITE, Event, Packet, Result, SchemeBlockMut};
use syscall::io::Mmio;

use arrayvec::ArrayVec;
use log::{debug, error, info, warn, trace};

use self::nvme::{InterruptMethod, Nvme};
use self::scheme::DiskScheme;

mod nvme;
mod scheme;

#[derive(Default)]
pub struct Bar {
    ptr: NonNull<u8>,
    physical: usize,
    bar_size: usize,
}
impl Bar {
    pub fn allocate(bar: usize, bar_size: usize) -> Result<Self> {
        Ok(Self {
            ptr: NonNull::new(syscall::physmap(bar, bar_size, PHYSMAP_NO_CACHE | PHYSMAP_WRITE)? as *mut u8).expect("Mapping a BAR resulted in a nullptr"),
            physical: bar,
            bar_size,
        })
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        let _ = syscall::physunmap(self.physical);
    }
}

#[derive(Default)]
pub struct AllocatedBars(pub [Mutex<Option<Bar>>; 6]);

/// Get the most optimal yet functional interrupt 
fn get_int_method(pcid_handle: &mut PcidServerHandle, function: &PciFunction, nvme: &mut Nvme, allocated_bars: &AllocatedBars) -> Result<InterruptMethod> {

    let features = pcid_handle.fetch_all_features().unwrap();

    let has_msi = features.iter().any(|(feature, _)| feature.is_msi());
    let has_msix = features.iter().any(|(feature, _)| feature.is_msix());

    // TODO: Allocate more than one vector when possible and useful.
    if has_msix {
        // Extended message signaled interrupts.
        use pcid_interface::msi::MsixTableEntry;
        use self::nvme::MsixCfg;

        let mut capability_struct = match pcid_handle.feature_info(PciFeature::MsiX).unwrap() {
            PciFeatureInfo::MsiX(msix) => msix,
            _ => unreachable!(),
        };
        fn bar_base(allocated_bars: &AllocatedBars, function: &PciFunction, bir: u8) -> Result<NonNull<u8>> {
            let bir = usize::from(bir);
            let bar_guard = allocated_bars.0[bir].lock().unwrap();
            match &mut *bar_guard {
                &mut Some(ref bar) => Ok(bar.ptr),
                bar_to_set @ &mut None => {
                    let bar = match function.bars[bir] {
                        PciBar::Memory(addr) => addr,
                        other => panic!("Expected memory BAR, found {:?}", other),
                    };
                    let bar_size = function.bar_sizes[bir];

                    let bar = Bar::allocate(bar as usize, bar_size as usize)?;
                    *bar_to_set = Some(bar);
                    Ok(bar_to_set.as_ref().unwrap().ptr)
                }
            }
        }
        let table_bar_base: *mut u8 = bar_base(allocated_bars, function, capability_struct.table_bir())?.as_ptr();
        let pba_bar_base: *mut u8 = bar_base(allocated_bars, function, capability_struct.pba_bir())?.as_ptr();
        let table_base = unsafe { table_bar_base.offset(capability_struct.table_offset() as isize) };
        let pba_base = unsafe { pba_bar_base.offset(capability_struct.pba_offset() as isize) };

        let vector_count = capability_struct.table_size();
        let table_entries: &'static mut [MsixTableEntry] = unsafe { slice::from_raw_parts_mut(table_base as *mut MsixTableEntry, vector_count as usize) };
        let pba_entries: &'static mut [Mmio<u64>] = unsafe { slice::from_raw_parts_mut(table_base as *mut Mmio<u64>, (vector_count as usize + 63) / 64) };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for table_entry in table_entries {
            table_entry.mask();
        }

        pcid_handle.enable_feature(PciFeature::MsiX).unwrap();
        capability_struct.set_msix_enabled(true); // only affects our local mirror of the cap

        // We don't allocate any vectors yet; that's later done when we get into
        // submission/completion queues.

        Ok(InterruptMethod::MsiX(MsixCfg {
            cap: capability_struct,
            table: table_entries,
            pba: pba_entries,
        }))
    } else if has_msi {
        // Message signaled interrupts.
        let capability_struct = match pcid_handle.feature_info(PciFeature::Msi).unwrap() {
            PciFeatureInfo::Msi(msi) => msi,
            _ => unreachable!(),
        };
        // We don't enable MSI until needed.
        Ok(InterruptMethod::Msi(capability_struct))
    } else if function.legacy_interrupt_pin.is_some() {
        // INTx# pin based interrupts.
        Ok(InterruptMethod::Intx)
    } else {
        // No interrupts at all
        todo!("handling of no interrupts")
    }
}

fn main() {
    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } != 0 {
        return;
    }

    let mut pcid_handle = PcidServerHandle::connect_default().expect("nvmed: failed to setup channel to pcid");
    let pci_config = pcid_handle.fetch_config().expect("nvmed: failed to fetch config");

    let bar = match pci_config.func.bars[0] {
        PciBar::Memory(mem) => mem,
        other => panic!("received a non-memory BAR ({:?})", other),
    };
    let bar_size = pci_config.func.bar_sizes[0];
    let irq = pci_config.func.legacy_interrupt_line;

    let mut name = pci_config.func.name();
    name.push_str("_nvme");

    info!("NVME PCI CONFIG: {:?}", pci_config);

    let allocated_bars = AllocatedBars::default();

    let address = unsafe {
        syscall::physmap(bar as usize, bar_size as usize, PHYSMAP_WRITE | PHYSMAP_NO_CACHE)
            .expect("nvmed: failed to map address")
    };
    *allocated_bars.0[0].lock().unwrap() = Some(Bar { physical: bar as usize, bar_size: bar_size as usize, ptr: NonNull::new(address as *mut u8).expect("Physmapping BAR gave nullptr") });
    {
        let event_fd = syscall::open("event:", syscall::O_RDWR | syscall::O_CLOEXEC)
            .expect("nvmed: failed to open event queue");
        let mut event_file = unsafe { File::from_raw_fd(event_fd as RawFd) };

        let irq_fd = syscall::open(
            &format!("irq:{}", irq),
            syscall::O_RDWR | syscall::O_NONBLOCK | syscall::O_CLOEXEC
        ).expect("nvmed: failed to open irq file");
        syscall::write(event_fd, &syscall::Event {
            id: irq_fd,
            flags: syscall::EVENT_READ,
            data: 0,
        }).expect("nvmed: failed to watch irq file events");
        let mut irq_file = unsafe { File::from_raw_fd(irq_fd as RawFd) };

        let scheme_name = format!("disk/{}", name);
        let socket_fd = syscall::open(
            &format!(":{}", scheme_name),
            syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK | syscall::O_CLOEXEC
        ).expect("nvmed: failed to create disk scheme");
        syscall::write(event_fd, &syscall::Event {
            id: socket_fd,
            flags: syscall::EVENT_READ,
            data: 1,
        }).expect("nvmed: failed to watch disk scheme events");
        let mut socket_file = unsafe { File::from_raw_fd(socket_fd as RawFd) };

        syscall::setrens(0, 0).expect("nvmed: failed to enter null namespace");

        let (reactor_sender, reactor_receiver) = crossbeam_channel::unbounded();
        let mut nvme = Nvme::new(address, interrupt_method, pcid_handle, reactor_sender).expect("nvmed: failed to allocate driver data");
        let nvme = Arc::new(nvme);
        unsafe { nvme.init() }
        nvme::cq_reactor::start_cq_reactor_thread(nvme);
        let namespaces = unsafe { nvme.init_with_queues() };
        let mut scheme = DiskScheme::new(scheme_name, nvme, namespaces);
        let mut todo = Vec::new();
        'events: loop {
            let mut event = Event::default();
            if event_file.read(&mut event).expect("nvmed: failed to read event queue") == 0 {
                break;
            }

            match event.data {
                0 => {
                    let mut irq = [0; 8];
                    if irq_file.read(&mut irq).expect("nvmed: failed to read irq file") >= irq.len() {
                        if scheme.irq() {
                            irq_file.write(&irq).expect("nvmed: failed to write irq file");
                        }
                    }
                },
                1 => loop {
                    let mut packet = Packet::default();
                    match socket_file.read(&mut packet) {
                        Ok(0) => break 'events,
                        Ok(_) => (),
                        Err(err) => match err.kind() {
                            ErrorKind::WouldBlock => break,
                            _ => Err(err).expect("nvmed: failed to read disk scheme"),
                        }
                    }
                    todo.push(packet);
                },
                unknown => {
                    panic!("nvmed: unknown event data {}", unknown);
                },
            }

            let mut i = 0;
            while i < todo.len() {
                if let Some(a) = scheme.handle(&todo[i]) {
                    let mut packet = todo.remove(i);
                    packet.a = a;
                    socket_file.write(&packet).expect("nvmed: failed to write disk scheme");
                } else {
                    i += 1;
                }
            }
        }

        //TODO: destroy NVMe stuff
    }
}
