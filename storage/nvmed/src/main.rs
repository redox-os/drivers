#![cfg_attr(target_arch = "aarch64", feature(stdarch_arm_hints))] // Required for yield instruction
#![feature(int_roundings)]

use std::convert::TryInto;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::{slice, usize};

use libredox::flag;
use pcid_interface::{PciFeature, PciFeatureInfo, PciFunction, PciFunctionHandle};
use redox_scheme::{CallRequest, RequestKind, SignalBehavior, Socket, V2};
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
    bar_size: usize,
}
impl Bar {
    pub fn new(ptr: *mut (), bar_size: usize) -> Result<Self> {
        Ok(Self {
            ptr: NonNull::new(ptr.cast::<u8>()).expect("Mapping a BAR resulted in a nullptr"),
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
    pcid_handle: &mut PciFunctionHandle,
    function: &PciFunction,
    allocated_bars: &AllocatedBars,
) -> Result<(InterruptMethod, InterruptSources)> {
    log::trace!("Begin get_int_method");
    use pcid_interface::irq_helpers;

    let features = pcid_handle.fetch_all_features().unwrap();

    let has_msi = features.iter().any(|feature| feature.is_msi());
    let has_msix = features.iter().any(|feature| feature.is_msix());

    // TODO: Allocate more than one vector when possible and useful.
    if has_msix {
        // Extended message signaled interrupts.
        use self::nvme::MappedMsixRegs;
        use pcid_interface::msi::MsixTableEntry;

        let msix_info = match pcid_handle.feature_info(PciFeature::MsiX).unwrap() {
            PciFeatureInfo::MsiX(msix) => msix,
            _ => unreachable!(),
        };
        msix_info.validate(function.bars);
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
                    let (ptr, bar_size) = unsafe { function.bars[bir].physmap_mem("nvmed") };

                    let bar = Bar::new(ptr, bar_size)?;
                    *bar_to_set = Some(bar);
                    Ok(bar_to_set.as_ref().unwrap().ptr)
                }
            }
        }
        let table_bar_base: *mut u8 =
            bar_base(allocated_bars, function, msix_info.table_bar)?.as_ptr();
        let table_base =
            unsafe { table_bar_base.offset(msix_info.table_offset as isize) };

        let vector_count = msix_info.table_size;
        let table_entries: &'static mut [MsixTableEntry] = unsafe {
            slice::from_raw_parts_mut(table_base as *mut MsixTableEntry, vector_count as usize)
        };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for table_entry in table_entries.iter_mut() {
            table_entry.mask();
        }

        pcid_handle.enable_feature(PciFeature::MsiX).unwrap();

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

        let interrupt_method = InterruptMethod::MsiX(MappedMsixRegs {
            info: msix_info,
            table: table_entries,
        });
        let interrupt_sources =
            InterruptSources::MsiX(std::iter::once((msix_vector_number, irq_handle)).collect());

        Ok((interrupt_method, interrupt_sources))
    } else if has_msi {
        // Message signaled interrupts.
        let msi_info = match pcid_handle.feature_info(PciFeature::Msi).unwrap() {
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

        let interrupt_method = InterruptMethod::Msi {
            msi_info,
            log2_multiple_message_enabled: 0,
        };
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
    pcid_handle: &mut PciFunctionHandle,
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
        PciFunctionHandle::connect_default().expect("nvmed: failed to setup channel to pcid");
    let pci_config = pcid_handle.config();

    let scheme_name = format!("disk.{}-nvme", pci_config.func.name());

    let _logger_ref = setup_logging(&scheme_name);

    log::debug!("NVME PCI CONFIG: {:?}", pci_config);

    let allocated_bars = AllocatedBars::default();

    let bar = &pci_config.func.bars[0];
    let (address, bar_size) = unsafe { bar.physmap_mem("nvmed") };

    *allocated_bars.0[0].lock().unwrap() = Some(Bar {
        bar_size,
        ptr: NonNull::new(address.cast::<u8>()).expect("Physmapping BAR gave nullptr"),
    });

    let socket = Socket::<V2>::create(&scheme_name).expect("nvmed: failed to create disk scheme");

    daemon.ready().expect("nvmed: failed to signal readiness");

    let (reactor_sender, reactor_receiver) = crossbeam_channel::unbounded();
    let (interrupt_method, interrupt_sources) =
        get_int_method(&mut pcid_handle, &pci_config.func, &allocated_bars)
            .expect("nvmed: failed to find a suitable interrupt method");
    let mut nvme = Nvme::new(
        address as usize,
        interrupt_method,
        pcid_handle,
        reactor_sender,
    )
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
        // TODO: Use a proper event queue once interrupt support is back.
        match socket.next_request(SignalBehavior::Restart).expect("nvmed: failed to read disk scheme") {
            None => {
                break;
            },
            Some(req) => if let RequestKind::Call(c) = req.kind() {
                todo.push(c);
            } else {
                // TODO: cancellation
                continue;
            }
        }

        let mut i = 0;
        while i < todo.len() {
            if let Some(resp) = todo[i].handle_scheme_block_mut(&mut scheme) {
                let _req = todo.remove(i);
                socket.write_response(resp, SignalBehavior::Restart)
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
