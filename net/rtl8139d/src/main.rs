#![feature(int_roundings)]

use std::cell::RefCell;
use std::convert::{Infallible, TryInto};
use std::fs::File;
use std::io::{Read, Result, Write};
use std::os::unix::io::AsRawFd;
use std::ptr::NonNull;
use std::rc::Rc;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::allocate_single_interrupt_vector_for_msi;
use pcid_interface::irq_helpers::read_bsp_apic_id;
use pcid_interface::msi::{MsixInfo, MsixTableEntry};
use pcid_interface::{
    MsiSetFeatureInfo, PciFeature, PciFeatureInfo, PciFunctionHandle, SetFeatureInfo,
    SubdriverArguments,
};
use syscall::EventFlags;

pub mod device;

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

pub struct MappedMsixRegs {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub info: MsixInfo,
}

impl MappedMsixRegs {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().offset(k as isize)
    }
    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.info.table_size as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> File {
    let pci_config = pcid_handle.config();

    let all_pci_features = pcid_handle
        .fetch_all_features()
        .expect("rtl8139d: failed to fetch pci features");
    log::info!("PCI FEATURES: {:?}", all_pci_features);

    let has_msi = all_pci_features.iter().any(|feature| feature.is_msi());
    let has_msix = all_pci_features.iter().any(|feature| feature.is_msix());

    if has_msi && !has_msix {
        let capability = match pcid_handle
            .feature_info(PciFeature::Msi)
            .expect("rtl8139d: failed to retrieve the MSI capability structure from pcid")
        {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("rtl8139d: failed to read BSP apic id");
        let (msg_addr_and_data, interrupt_handle) =
            allocate_single_interrupt_vector_for_msi(destination_id);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address_and_data: Some(msg_addr_and_data),
            mask_bits: None,
        };
        pcid_handle
            .set_feature_info(SetFeatureInfo::Msi(set_feature_info))
            .expect("rtl8139d: failed to set feature info");

        pcid_handle
            .enable_feature(PciFeature::Msi)
            .expect("rtl8139d: failed to enable MSI");
        log::info!("Enabled MSI");

        interrupt_handle
    } else if has_msix {
        let msix_info = match pcid_handle
            .feature_info(PciFeature::MsiX)
            .expect("rtl8139d: failed to retrieve the MSI-X capability structure from pcid")
        {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        msix_info.validate(pci_config.func.bars);

        let bar_address = unsafe { pcid_handle.map_bar(msix_info.table_bar).expect("rtl8139d") }
            .ptr
            .as_ptr() as usize;

        let virt_table_base =
            (bar_address + msix_info.table_offset as usize) as *mut MsixTableEntry;

        let mut info = MappedMsixRegs {
            virt_table_base: NonNull::new(virt_table_base).unwrap(),
            info: msix_info,
        };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("rtl8139d: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) =
                allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            interrupt_handle
        };

        pcid_handle
            .enable_feature(PciFeature::MsiX)
            .expect("rtl8139d: failed to enable MSI-X");
        log::info!("Enabled MSI-X");

        method
    } else if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        irq.irq_handle("rtl8139d")
    } else {
        panic!("rtl8139d: no interrupts supported at all")
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> File {
    let pci_config = pcid_handle.config();

    if let Some(irq) = pci_config.func.legacy_interrupt_line {
        // legacy INTx# interrupt pins.
        irq.irq_handle("rtl8139d")
    } else {
        panic!("rtl8139d: no interrupts supported at all")
    }
}

fn find_bar(pci_config: &SubdriverArguments) -> Option<(usize, usize)> {
    // RTL8139 uses BAR2, RTL8169 uses BAR1, search in that order
    for &barnum in &[2, 1] {
        match pci_config.func.bars[barnum] {
            pcid_interface::PciBar::Memory32 { addr, size } => {
                return Some((addr.try_into().unwrap(), size.try_into().unwrap()))
            }
            pcid_interface::PciBar::Memory64 { addr, size } => {
                return Some((addr.try_into().unwrap(), size.try_into().unwrap()))
            }
            other => log::warn!("BAR {} is {:?} instead of memory BAR", barnum, other),
        }
    }
    None
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    common::setup_logging(
        "net",
        "pcie",
        "rtl8139",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    let mut pcid_handle =
        PciFunctionHandle::connect_default().expect("rtl8139d: failed to setup channel to pcid");

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_rtl8139");

    let (bar_ptr, bar_size) = find_bar(&pci_config).expect("rtl8139d: failed to find BAR");
    log::info!(" + RTL8139 {}", pci_config.func.display());

    let address = unsafe {
        common::physmap(
            bar_ptr,
            bar_size,
            common::Prot::RW,
            common::MemoryType::Uncacheable,
        )
        .expect("rtl8139d: failed to map address") as usize
    };

    //TODO: MSI-X
    let mut irq_file = get_int_method(&mut pcid_handle);

    let device =
        unsafe { device::Rtl8139::new(address).expect("rtl8139d: failed to allocate device") };

    let mut scheme = NetworkScheme::new(device, format!("network.{name}"));

    user_data! {
        enum Source {
            Irq,
            Scheme,
        }
    }

    let mut event_queue =
        EventQueue::<Source>::new().expect("rtl8139d: Could not create event queue.");
    event_queue
        .subscribe(
            irq_file.as_raw_fd() as usize,
            Source::Irq,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            scheme.event_handle() as usize,
            Source::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    libredox::call::setrens(0, 0).expect("rtl8139d: failed to enter null namespace");

    daemon
        .ready()
        .expect("rtl8139d: failed to mark daemon as ready");

    scheme.tick().unwrap();

    for event in event_queue.map(|e| e.expect("rtl8139d: failed to get next event")) {
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
    redox_daemon::Daemon::new(daemon).expect("rtl8139d: failed to create daemon");
}
