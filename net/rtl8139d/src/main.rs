use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
use pcid_interface::irq_helpers::read_bsp_apic_id;
#[cfg(target_arch = "x86_64")]
use pcid_interface::irq_helpers::{
    allocate_first_msi_interrupt_on_bsp, allocate_single_interrupt_vector_for_msi,
};
use pcid_interface::{PciFeature, PciFeatureInfo, PciFunctionHandle};

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

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PciFunctionHandle) -> File {
    let pci_config = pcid_handle.config();

    let all_pci_features = pcid_handle.fetch_all_features();
    log::info!("PCI FEATURES: {:?}", all_pci_features);

    let has_msi = all_pci_features.iter().any(|feature| feature.is_msi());
    let has_msix = all_pci_features.iter().any(|feature| feature.is_msix());

    if has_msix {
        let msix_info = match pcid_handle.feature_info(PciFeature::MsiX) {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        let mut info = unsafe { msix_info.map_and_mask_all(pcid_handle) };

        // Allocate one msi vector.

        let method = {
            // primary interrupter
            let k = 0;

            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("rtl8139d: failed to read BSP apic id");
            let (msg_addr_and_data, interrupt_handle) =
                allocate_single_interrupt_vector_for_msi(destination_id);
            table_entry_pointer.write_addr_and_data(msg_addr_and_data);
            table_entry_pointer.unmask();

            interrupt_handle
        };

        pcid_handle.enable_feature(PciFeature::MsiX);
        log::debug!("Enabled MSI-X");

        method
    } else if has_msi {
        allocate_first_msi_interrupt_on_bsp(pcid_handle)
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

fn map_bar(pcid_handle: &mut PciFunctionHandle) -> *mut u8 {
    let config = pcid_handle.config();

    // RTL8139 uses BAR2, RTL8169 uses BAR1, search in that order
    for &barnum in &[2, 1] {
        match config.func.bars[usize::from(barnum)] {
            pcid_interface::PciBar::Memory32 { .. } | pcid_interface::PciBar::Memory64 { .. } => unsafe {
                return pcid_handle.map_bar(barnum).ptr.as_ptr();
            },
            other => log::warn!("BAR {} is {:?} instead of memory BAR", barnum, other),
        }
    }
    panic!("rtl8139d: failed to find BAR");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_rtl8139");

    common::setup_logging(
        "net",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    log::info!(" + RTL8139 {}", pci_config.func.display());

    let bar = map_bar(&mut pcid_handle);

    //TODO: MSI-X
    let mut irq_file = get_int_method(&mut pcid_handle);

    let device =
        unsafe { device::Rtl8139::new(bar as usize).expect("rtl8139d: failed to allocate device") };

    let mut scheme = NetworkScheme::new(device, format!("network.{name}"));

    user_data! {
        enum Source {
            Irq,
            Scheme,
        }
    }

    let event_queue = EventQueue::<Source>::new().expect("rtl8139d: Could not create event queue.");
    event_queue
        .subscribe(
            irq_file.as_raw_fd() as usize,
            Source::Irq,
            event::EventFlags::READ,
        )
        .unwrap();
    event_queue
        .subscribe(
            scheme.event_handle().raw(),
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
