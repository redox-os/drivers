use std::cell::RefCell;
use std::convert::{Infallible, TryInto};
use std::fs::File;
use std::io::{Read, Result, Write};
use std::os::unix::io::AsRawFd;
use std::ptr::NonNull;
use std::rc::Rc;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
use pcid_interface::{MsiSetFeatureInfo, PcidServerHandle, PciFeature, PciFeatureInfo, SetFeatureInfo, SubdriverArguments};
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::allocate_single_interrupt_vector_for_msi;
use pcid_interface::irq_helpers::read_bsp_apic_id;
use pcid_interface::msi::{MsixCapability, MsixTableEntry};
use redox_log::{RedoxLogger, OutputBuilder};
use syscall::EventFlags;

pub mod device;

fn setup_logging() -> Option<&'static RedoxLogger> {
    let mut logger = RedoxLogger::new()
        .with_output(
            OutputBuilder::stderr()
                .with_filter(log::LevelFilter::Info) // limit global output to important info
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        );

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("net", "pcie", "rtl8168.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create rtl8168.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("net", "pcie", "rtl8168.ansi.log") {
        Ok(b) => logger = logger.with_output(
            b.with_filter(log::LevelFilter::Info)
                .with_ansi_escape_codes()
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create rtl8168.ansi.log: {}", error),
    }

    match logger.enable() {
        Ok(logger_ref) => {
            eprintln!("rtl8168d: enabled logger");
            Some(logger_ref)
        }
        Err(error) => {
            eprintln!("rtl8168d: failed to set default logger: {}", error);
            None
        }
    }
}

use std::ops::{Add, Div, Rem};
pub fn div_round_up<T>(a: T, b: T) -> T
where
    T: Add<Output = T> + Div<Output = T> + Rem<Output = T> + PartialEq + From<u8> + Copy,
{
    if a % b != T::from(0u8) {
        a / b + T::from(1u8)
    } else {
        a / b
    }
}

pub struct MsixInfo {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub capability: MsixCapability,
}

impl MsixInfo {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().offset(k as isize)
    }
    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PcidServerHandle) -> File {
    let pci_config = pcid_handle.fetch_config().expect("rtl8168d: failed to fetch config");

    let all_pci_features = pcid_handle.fetch_all_features().expect("rtl8168d: failed to fetch pci features");
    log::info!("PCI FEATURES: {:?}", all_pci_features);

    let (has_msi, mut msi_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msi(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));
    let (has_msix, mut msix_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msix(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));

    if has_msi && !msi_enabled && !has_msix {
        msi_enabled = true;
    }
    if has_msix && !msix_enabled {
        msix_enabled = true;
    }

    if msi_enabled && !msix_enabled {
        let capability = match pcid_handle.feature_info(PciFeature::Msi).expect("rtl8168d: failed to retrieve the MSI capability structure from pcid") {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("rtl8168d: failed to read BSP apic id");
        let (msg_addr_and_data, interrupt_handle) = allocate_single_interrupt_vector_for_msi(destination_id);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address_and_data: Some(msg_addr_and_data),
            mask_bits: None,
        };
        pcid_handle.set_feature_info(SetFeatureInfo::Msi(set_feature_info)).expect("rtl8168d: failed to set feature info");

        pcid_handle.enable_feature(PciFeature::Msi).expect("rtl8168d: failed to enable MSI");
        log::info!("Enabled MSI");

        interrupt_handle
    } else if msix_enabled {
        let capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("rtl8168d: failed to retrieve the MSI-X capability structure from pcid") {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        capability.validate(pci_config.func.bars);

        let bar = &pci_config.func.bars[capability.table_bir() as usize];
        let bar_address = unsafe { bar.physmap_mem("rtl8168d") } as usize;

        let virt_table_base = (bar_address + capability.table_offset() as usize) as *mut MsixTableEntry;

        let mut info = MsixInfo {
            virt_table_base: NonNull::new(virt_table_base).unwrap(),
            capability,
        };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("rtl8168d: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) =
                allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            interrupt_handle
        };

        pcid_handle.enable_feature(PciFeature::MsiX).expect("rtl8168d: failed to enable MSI-X");
        log::info!("Enabled MSI-X");

        method
    } else if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        irq.irq_handle("rtl8168d")
    } else {
        panic!("rtl8168d: no interrupts supported at all")
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(pcid_handle: &mut PcidServerHandle) -> File {
    let pci_config = pcid_handle.fetch_config().expect("rtl8168d: failed to fetch config");

    if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        irq.irq_handle("rtl8168d")
    } else {
        panic!("rtl8168d: no interrupts supported at all")
    }
}

fn find_bar(pci_config: &SubdriverArguments) -> Option<(usize, usize)> {
    // RTL8168 uses BAR2, RTL8169 uses BAR1, search in that order
    for &barnum in &[2, 1] {
        match pci_config.func.bars[barnum] {
            pcid_interface::PciBar::Memory32 { addr, size } => return Some((
                addr.try_into().unwrap(),
                size.try_into().unwrap()
            )),
            pcid_interface::PciBar::Memory64 { addr, size } => return Some((
                addr.try_into().unwrap(),
                size.try_into().unwrap()
            )),
            other => log::warn!("BAR {} is {:?} instead of memory BAR", barnum, other),
        }
    }
    None
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let _logger_ref = setup_logging();

    let mut pcid_handle = PcidServerHandle::connect_default().expect("rtl8168d: failed to setup channel to pcid");

    let pci_config = pcid_handle.fetch_config().expect("rtl8168d: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_rtl8168");

    let (bar_ptr, bar_size) = find_bar(&pci_config).expect("rtl8168d: failed to find BAR");
    log::info!(" + RTL8168 {}", pci_config.func.display());

    let address = unsafe {
        common::physmap(bar_ptr, bar_size, common::Prot::RW, common::MemoryType::Uncacheable)
            .expect("rtl8168d: failed to map address") as usize
    };

    //TODO: MSI-X
    let mut irq_file = get_int_method(&mut pcid_handle);

    let device =
        unsafe { device::Rtl8168::new(address).expect("rtl8168d: failed to allocate device") };

    let mut scheme = NetworkScheme::new(device, format!("network.{name}"));

    user_data! {
        enum Source {
            Irq,
            Scheme,
        }
    }

    let mut event_queue = EventQueue::<Source>::new().expect("rtl8168d: Could not create event queue.");
    event_queue.subscribe(irq_file.as_raw_fd() as usize, Source::Irq, event::EventFlags::READ).unwrap();
    event_queue.subscribe(scheme.event_handle() as usize, Source::Scheme, event::EventFlags::READ).unwrap();

    libredox::call::setrens(0, 0).expect("rtl8168d: failed to enter null namespace");

    daemon
        .ready()
        .expect("rtl8168d: failed to mark daemon as ready");

    scheme.tick().unwrap();

    for event in event_queue.map(|e| e.expect("rtl8168d: failed to get next event")) {
        match event.user_data {
            Source::Irq => {
                let mut irq = [0; 8];
                irq_file.read(&mut irq).unwrap();
                //TODO: This may be causing spurious interrupts
                if unsafe { scheme.adapter_mut().irq() } {
                    irq_file.write(&mut irq).unwrap();

                    scheme.tick().unwrap();
                }
            }
            Source::Scheme => {
                scheme.tick().unwrap();
            }
        }
    }
    unreachable!()
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("rtl8168d: failed to create daemon");
}
