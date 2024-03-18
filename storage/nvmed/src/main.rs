#![cfg_attr(target_arch = "aarch64", feature(stdsimd))] // Required for yield instruction
#![feature(int_roundings)]

use std::convert::TryInto;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::{slice, usize};

use libredox::flag;
use pcid_interface::{PciFeature, PciFeatureInfo, PciFunction, PcidServerHandle};
use syscall::{
    Event, Mmio, Packet, Result, SchemeBlockMut,
    PAGE_SIZE,
};
use redox_log::{OutputBuilder, RedoxLogger};

use self::nvme::{InterruptMethod, InterruptSources, Nvme};
use self::scheme::DiskScheme;

mod nvme;
mod scheme;

/// A wrapper for a BAR allocation.
pub struct Bar {
    ptr: NonNull<u8>,
    physical: usize,
    bar_size: usize,
}
impl Bar {
    pub fn allocate(bar: usize, bar_size: usize) -> Result<Self> {
        Ok(Self {
            ptr: NonNull::new(
                unsafe { common::physmap(
                    bar,
                    bar_size,
                    common::Prot { read: true, write: true },
                    common::MemoryType::Uncacheable,
                )? as *mut u8 },
            )
            .expect("Mapping a BAR resulted in a nullptr"),
            physical: bar,
            bar_size,
        })
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        let _ = unsafe {
            libredox::call::munmap(
                self.ptr.as_ptr().cast(),
                self.bar_size.next_multiple_of(PAGE_SIZE),
            )
        };
    }
}

/// The PCI BARs that may be allocated.
#[derive(Default)]
pub struct AllocatedBars(pub [Mutex<Option<Bar>>; 6]);

/// Get the most optimal yet functional interrupt mechanism: either (in the order of preference):
/// MSI-X, MSI, and INTx# pin. Returns both runtime interrupt structures (MSI/MSI-X capability
/// structures), and the handles to the interrupts.
#[cfg(target_arch = "x86_64")]
fn get_int_method(
    pcid_handle: &mut PcidServerHandle,
    function: &PciFunction,
    allocated_bars: &AllocatedBars,
) -> Result<(InterruptMethod, InterruptSources)> {
    log::trace!("Begin get_int_method");
    use pcid_interface::irq_helpers;

    let features = pcid_handle.fetch_all_features().unwrap();

    let has_msi = features.iter().any(|(feature, _)| feature.is_msi());
    let has_msix = features.iter().any(|(feature, _)| feature.is_msix());

    // TODO: Allocate more than one vector when possible and useful.
    if has_msix {
        // Extended message signaled interrupts.
        use self::nvme::MsixCfg;
        use pcid_interface::msi::MsixTableEntry;

        let mut capability_struct = match pcid_handle.feature_info(PciFeature::MsiX).unwrap() {
            PciFeatureInfo::MsiX(msix) => msix,
            _ => unreachable!(),
        };
        capability_struct.validate(function.bars);
        fn bar_base(
            allocated_bars: &AllocatedBars,
            function: &PciFunction,
            bir: u8,
        ) -> Result<NonNull<u8>> {
            let bir = usize::from(bir);
            let mut bar_guard = allocated_bars.0[bir].lock().unwrap();
            match &mut *bar_guard {
                &mut Some(ref bar) => Ok(bar.ptr),
                bar_to_set @ &mut None => {
                    let (bar, bar_size) = function.bars[bir].expect_mem();

                    let bar = Bar::allocate(bar, bar_size)?;
                    *bar_to_set = Some(bar);
                    Ok(bar_to_set.as_ref().unwrap().ptr)
                }
            }
        }
        let table_bar_base: *mut u8 =
            bar_base(allocated_bars, function, capability_struct.table_bir())?.as_ptr();
        let table_base =
            unsafe { table_bar_base.offset(capability_struct.table_offset() as isize) };

        let vector_count = capability_struct.table_size();
        let table_entries: &'static mut [MsixTableEntry] = unsafe {
            slice::from_raw_parts_mut(table_base as *mut MsixTableEntry, vector_count as usize)
        };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for table_entry in table_entries.iter_mut() {
            table_entry.mask();
        }

        pcid_handle.enable_feature(PciFeature::MsiX).unwrap();
        capability_struct.set_msix_enabled(true); // only affects our local mirror of the cap

        let (msix_vector_number, irq_handle) = {
            let entry: &mut MsixTableEntry = &mut table_entries[0];

            let bsp_cpu_id =
                irq_helpers::read_bsp_apic_id().expect("nvmed: failed to read APIC ID");
            let (msg_addr_and_data, irq_handle) =
                irq_helpers::allocate_single_interrupt_vector_for_msi(bsp_cpu_id);
            entry.write_addr_and_data(msg_addr_and_data);
            entry.unmask();

            (0, irq_handle)
        };

        let interrupt_method = InterruptMethod::MsiX(MsixCfg {
            cap: capability_struct,
            table: table_entries,
        });
        let interrupt_sources =
            InterruptSources::MsiX(std::iter::once((msix_vector_number, irq_handle)).collect());

        Ok((interrupt_method, interrupt_sources))
    } else if has_msi {
        // Message signaled interrupts.
        let capability_struct = match pcid_handle.feature_info(PciFeature::Msi).unwrap() {
            PciFeatureInfo::Msi(msi) => msi,
            _ => unreachable!(),
        };

        let (msi_vector_number, irq_handle) = {
            use pcid_interface::{MsiSetFeatureInfo, SetFeatureInfo};

            let bsp_cpu_id =
                irq_helpers::read_bsp_apic_id().expect("nvmed: failed to read BSP APIC ID");
            let (msg_addr_and_data, irq_handle) =
                irq_helpers::allocate_single_interrupt_vector_for_msi(bsp_cpu_id);

            pcid_handle.set_feature_info(SetFeatureInfo::Msi(MsiSetFeatureInfo {
                message_address_and_data: Some(msg_addr_and_data),
                multi_message_enable: Some(0), // enable 2^0=1 vectors
                mask_bits: None,
            })).unwrap();

            (0, irq_handle)
        };

        let interrupt_method = InterruptMethod::Msi(capability_struct);
        let interrupt_sources =
            InterruptSources::Msi(std::iter::once((msi_vector_number, irq_handle)).collect());

        pcid_handle.enable_feature(PciFeature::Msi).unwrap();

        Ok((interrupt_method, interrupt_sources))
    } else if let Some(irq) = function.legacy_interrupt_line {
        // INTx# pin based interrupts.
        let irq_handle = irq.irq_handle("nvmed");
        Ok((InterruptMethod::Intx, InterruptSources::Intx(irq_handle)))
    } else {
        panic!("nvmed: no interrupts supported at all")
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(
    pcid_handle: &mut PcidServerHandle,
    function: &PciFunction,
    allocated_bars: &AllocatedBars,
) -> Result<(InterruptMethod, InterruptSources)> {
    if let Some(irq) = function.legacy_interrupt_line {
        // INTx# pin based interrupts.
        let irq_handle = irq.irq_handle("nvmed");
        Ok((InterruptMethod::Intx, InterruptSources::Intx(irq_handle)))
    } else {
        panic!("nvmed: no interrupts supported at all")
    }
}

fn setup_logging(name: &str) -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", &format!("{}.log", name)) {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("nvmed: failed to create log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", &format!("{}.ansi.log", name)) {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("nvmed: failed to create ansi log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("nvmed: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("nvmed: failed to set default logger: {}", error);
            None
        }
    }
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("nvmed: failed to daemonize");
}
fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("nvmed: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("nvmed: failed to fetch config");

    let scheme_name = format!("disk.{}-nvme", pci_config.func.name());

    let _logger_ref = setup_logging(&scheme_name);

    let bar = &pci_config.func.bars[0];
    let (bar_ptr, bar_size) = bar.expect_mem();

    log::debug!("NVME PCI CONFIG: {:?}", pci_config);

    let allocated_bars = AllocatedBars::default();

    let address = unsafe { bar.physmap_mem("nvmed") } as usize;
    *allocated_bars.0[0].lock().unwrap() = Some(Bar {
        physical: bar_ptr,
        bar_size,
        ptr: NonNull::new(address as *mut u8).expect("Physmapping BAR gave nullptr"),
    });

    let socket_fd = libredox::call::open(
        &format!(":{}", scheme_name),
        flag::O_RDWR | flag::O_CREAT | flag::O_CLOEXEC,
        0,
    )
    .expect("nvmed: failed to create disk scheme");
    let mut socket_file = unsafe { File::from_raw_fd(socket_fd as RawFd) };

    daemon.ready().expect("nvmed: failed to signal readiness");

    let (reactor_sender, reactor_receiver) = crossbeam_channel::unbounded();
    let (interrupt_method, interrupt_sources) =
        get_int_method(&mut pcid_handle, &pci_config.func, &allocated_bars)
            .expect("nvmed: failed to find a suitable interrupt method");
    let mut nvme = Nvme::new(address, interrupt_method, pcid_handle, reactor_sender)
        .expect("nvmed: failed to allocate driver data");
    unsafe { nvme.init() }
    log::debug!("Finished base initialization");
    let nvme = Arc::new(nvme);
    #[cfg(feature = "async")]
    let reactor_thread = nvme::cq_reactor::start_cq_reactor_thread(Arc::clone(&nvme), interrupt_sources, reactor_receiver);
    let namespaces = nvme.init_with_queues();

    libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    let mut scheme = DiskScheme::new(scheme_name, nvme, namespaces);
    let mut todo = Vec::new();
    loop {
        let mut packet = Packet::default();
        match socket_file.read(&mut packet) {
            Ok(0) => {
                break;
            },
            Ok(_) => {
                todo.push(packet);
            },
            Err(err) => match err.kind() {
                ErrorKind::WouldBlock => break,
                _ => Err(err).expect("nvmed: failed to read disk scheme"),
            },
        }

        let mut i = 0;
        while i < todo.len() {
            if let Some(a) = scheme.handle(&todo[i]) {
                let mut packet = todo.remove(i);
                packet.a = a;
                socket_file
                    .write(&packet)
                    .expect("nvmed: failed to write disk scheme");
            } else {
                i += 1;
            }
        }
    }

    //TODO: destroy NVMe stuff
    #[cfg(feature = "async")]
    reactor_thread.join().expect("nvmed: failed to join reactor thread");

    std::process::exit(0);
}
