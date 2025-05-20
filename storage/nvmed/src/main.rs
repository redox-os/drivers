#![cfg_attr(target_arch = "aarch64", feature(stdarch_arm_hints))] // Required for yield instruction
#![cfg_attr(target_arch = "riscv64", feature(riscv_ext_intrinsics))] // Required for pause instruction

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::{slice, usize};

use driver_block::{Disk, DiskScheme};
use pcid_interface::{PciFeature, PciFeatureInfo, PciFunction, PciFunctionHandle};
use syscall::Result;

use crate::nvme::NvmeNamespace;

use self::nvme::{InterruptMethod, InterruptSources, Nvme};

mod nvme;

/// Get the most optimal yet functional interrupt mechanism: either (in the order of preference):
/// MSI-X, MSI, and INTx# pin. Returns both runtime interrupt structures (MSI/MSI-X capability
/// structures), and the handles to the interrupts.
#[cfg(target_arch = "x86_64")]
fn get_int_method(
    pcid_handle: &mut PciFunctionHandle,
    function: &PciFunction,
) -> Result<(InterruptMethod, InterruptSources)> {
    log::trace!("Begin get_int_method");
    use pcid_interface::irq_helpers;

    let features = pcid_handle.fetch_all_features();

    let has_msi = features.iter().any(|feature| feature.is_msi());
    let has_msix = features.iter().any(|feature| feature.is_msix());

    // TODO: Allocate more than one vector when possible and useful.
    if has_msix {
        // Extended message signaled interrupts.
        use self::nvme::MappedMsixRegs;
        use pcid_interface::msi::MsixTableEntry;

        let msix_info = match pcid_handle.feature_info(PciFeature::MsiX) {
            PciFeatureInfo::MsiX(msix) => msix,
            _ => unreachable!(),
        };
        msix_info.validate(function.bars);
        fn bar_base(pcid_handle: &mut PciFunctionHandle, bir: u8) -> Result<NonNull<u8>> {
            Ok(unsafe { pcid_handle.map_bar(bir) }.ptr)
        }
        let table_bar_base: *mut u8 = bar_base(pcid_handle, msix_info.table_bar)?.as_ptr();
        let table_base = unsafe { table_bar_base.offset(msix_info.table_offset as isize) };

        let vector_count = msix_info.table_size;
        let table_entries: &'static mut [MsixTableEntry] = unsafe {
            slice::from_raw_parts_mut(table_base as *mut MsixTableEntry, vector_count as usize)
        };

        // Mask all interrupts in case some earlier driver/os already unmasked them (according to
        // the PCI Local Bus spec 3.0, they are masked after system reset).
        for table_entry in table_entries.iter_mut() {
            table_entry.mask();
        }

        pcid_handle.enable_feature(PciFeature::MsiX);

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

        log::trace!("Using MSI-X");
        Ok((interrupt_method, interrupt_sources))
    } else if has_msi {
        // Message signaled interrupts.
        let msi_info = match pcid_handle.feature_info(PciFeature::Msi) {
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
            }));

            (0, irq_handle)
        };

        let interrupt_method = InterruptMethod::Msi {
            msi_info,
            log2_multiple_message_enabled: 0,
        };
        let interrupt_sources =
            InterruptSources::Msi(std::iter::once((msi_vector_number, irq_handle)).collect());

        pcid_handle.enable_feature(PciFeature::Msi);

        log::trace!("Using MSI");
        Ok((interrupt_method, interrupt_sources))
    } else if let Some(irq) = function.legacy_interrupt_line {
        // INTx# pin based interrupts.
        let irq_handle = irq.irq_handle("nvmed");
        log::trace!("Using legacy interrupts");
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
) -> Result<(InterruptMethod, InterruptSources)> {
    if let Some(irq) = function.legacy_interrupt_line {
        // INTx# pin based interrupts.
        let irq_handle = irq.irq_handle("nvmed");
        Ok((InterruptMethod::Intx, InterruptSources::Intx(irq_handle)))
    } else {
        panic!("nvmed: no interrupts supported at all")
    }
}

struct NvmeDisk {
    nvme: Arc<Nvme>,
    ns: NvmeNamespace,
}

impl Disk for NvmeDisk {
    fn block_size(&self) -> u32 {
        self.ns.block_size.try_into().unwrap()
    }

    fn size(&self) -> u64 {
        self.ns.blocks * self.ns.block_size
    }

    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
        self.nvme.namespace_read(&self.ns, block, buffer).await
    }

    async fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
        self.nvme.namespace_write(&self.ns, block, buffer).await
    }
}

fn time_arm(time_handle: &mut File, secs: i64) -> io::Result<()> {
    let mut time_buf = [0_u8; core::mem::size_of::<libredox::data::TimeSpec>()];
    if time_handle.read(&mut time_buf)? < time_buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "time read too small",
        ));
    }

    match libredox::data::timespec_from_mut_bytes(&mut time_buf) {
        time => {
            time.tv_sec += secs;
        }
    }
    time_handle.write(&time_buf)?;
    Ok(())
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("nvmed: failed to daemonize");
}
fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let scheme_name = format!("disk.{}-nvme", pci_config.func.name());

    common::setup_logging(
        "disk",
        "pcie",
        &scheme_name,
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    log::debug!("NVME PCI CONFIG: {:?}", pci_config);

    let address = unsafe { pcid_handle.map_bar(0).ptr };

    let (interrupt_method, interrupt_sources) = get_int_method(&mut pcid_handle, &pci_config.func)
        .expect("nvmed: failed to find a suitable interrupt method");
    let mut nvme = Nvme::new(address.as_ptr() as usize, interrupt_method, pcid_handle)
        .expect("nvmed: failed to allocate driver data");

    unsafe { nvme.init() }
    log::debug!("Finished base initialization");
    let nvme = Arc::new(nvme);

    let executor = {
        let (intx, (iv, irq_handle)) = match interrupt_sources {
            InterruptSources::Msi(mut vectors) => (
                false,
                vectors.pop_first().map(|(a, b)| (u16::from(a), b)).unwrap(),
            ),
            InterruptSources::MsiX(mut vectors) => (false, vectors.pop_first().unwrap()),
            InterruptSources::Intx(file) => (true, (0, file)),
        };
        nvme::executor::init(Arc::clone(&nvme), iv, intx, irq_handle)
    };

    let mut time_handle = File::open(&format!("/scheme/time/{}", libredox::flag::CLOCK_MONOTONIC))
        .expect("failed to open time handle");

    let mut time_events = Box::pin(
        executor.register_external_event(time_handle.as_raw_fd() as usize, event::EventFlags::READ),
    );

    // Try to init namespaces for 5 seconds
    time_arm(&mut time_handle, 5).expect("failed to arm timer");
    let namespaces = executor.block_on(async {
        let namespaces_future = nvme.init_with_queues();
        let time_future = time_events.as_mut().next();
        futures::pin_mut!(namespaces_future);
        futures::pin_mut!(time_future);
        match futures::future::select(namespaces_future, time_future).await {
            futures::future::Either::Left((namespaces, _)) => namespaces,
            futures::future::Either::Right(_) => panic!("timeout on init"),
        }
    });
    log::debug!("Initialized!");

    let scheme = Rc::new(RefCell::new(DiskScheme::new(
        Some(daemon),
        scheme_name,
        namespaces
            .into_iter()
            .map(|(k, ns)| {
                (
                    k,
                    NvmeDisk {
                        nvme: nvme.clone(),
                        ns,
                    },
                )
            })
            .collect(),
        &*executor,
    )));

    let mut scheme_events = Box::pin(executor.register_external_event(
        scheme.borrow().event_handle().raw(),
        event::EventFlags::READ,
    ));

    libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    log::info!("Starting to listen for scheme events");

    executor.block_on(async {
        loop {
            log::trace!("new event iteration");
            if let Err(err) = scheme.borrow_mut().tick().await {
                log::error!("scheme error: {err}");
            }
            let _ = scheme_events.as_mut().next().await;
        }
    });

    //TODO: destroy NVMe stuff

    std::process::exit(0);
}
