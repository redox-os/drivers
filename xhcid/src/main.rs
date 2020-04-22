#[macro_use]
extern crate bitflags;

use std::convert::{TryFrom, TryInto};
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::env;

use pcid_interface::{MsiSetFeatureInfo, PcidServerHandle, PciFeature, PciFeatureInfo, SetFeatureInfo};
use pcid_interface::irq_helpers::{read_bsp_apic_id, allocate_single_interrupt_vector};
use pcid_interface::msi::{MsiCapability, MsixCapability, MsixTableEntry};

use event::{Event, EventQueue};
use log::info;
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::flag::{CloneFlags, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::scheme::Scheme;
use syscall::io::Io;

use crate::xhci::{InterruptMethod, Xhci};

// Declare as pub so that no warnings appear due to parts of the interface code not being used by
// the driver. Since there's also a dedicated crate for the driver interface, those warnings don't
// mean anything.
pub mod driver_interface;

mod usb;
mod xhci;

async fn handle_packet(hci: Arc<Xhci>, packet: Packet) -> Packet {
    todo!()
}

fn main() {
    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

    // Daemonize
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    match redox_log::RedoxLogger::new("usb", "host", "xhci.log") {
        Ok(logger) => match logger.with_stdout_mirror().enable() {
            Ok(_) => {
                println!("xhcid: enabled logger");
                log::set_max_level(log::LevelFilter::Trace);
            }
            Err(error) => eprintln!("xhcid: failed to set default logger: {}", error),
        }
        Err(error) => eprintln!("xhcid: failed to initialize logger: {}", error),
    }

    let mut pcid_handle = PcidServerHandle::connect_default().expect("xhcid: failed to setup channel to pcid");
    let pci_config = pcid_handle.fetch_config().expect("xhcid: failed to fetch config");
    info!("XHCI PCI CONFIG: {:?}", pci_config);

    let bar = pci_config.func.bars[0];
    let irq = pci_config.func.legacy_interrupt_line;

    let bar_ptr = match bar {
        pcid_interface::PciBar::Memory(ptr) => ptr,
        other => panic!("Expected memory bar, found {}", other),
    };

    let address = unsafe {
        syscall::physmap(bar_ptr as usize, 65536, PHYSMAP_WRITE | PHYSMAP_NO_CACHE)
            .expect("xhcid: failed to map address")
    };

    let all_pci_features = pcid_handle.fetch_all_features().expect("xhcid: failed to fetch pci features");
    info!("XHCI PCI FEATURES: {:?}", all_pci_features);

    let (has_msi, mut msi_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msi(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));
    let (has_msix, mut msix_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msix(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));

    if has_msi && !msi_enabled && !has_msix {
        msi_enabled = true;
    }
    if has_msix && !msix_enabled {
        msix_enabled = true;
    }

    let (mut irq_file, interrupt_method) = if msi_enabled && !msix_enabled {
        use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

        let mut capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("xhcid: failed to retrieve the MSI capability structure from pcid") {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
        let lapic_id = u8::try_from(destination_id).expect("CPU id didn't fit inside u8");
        let msg_addr = x86_64_msix::message_address(lapic_id, false, false, 0b00);

        let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id).expect("xhcid: failed to allocate interrupt vector").expect("xhcid: no interrupt vectors left");
        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address: Some(msg_addr),
            message_upper_address: Some(0),
            message_data: Some(msg_data as u16),
            mask_bits: None,
        };
        pcid_handle.set_feature_info(SetFeatureInfo::Msi(set_feature_info)).expect("xhcid: failed to set feature info");

        pcid_handle.enable_feature(PciFeature::Msi).expect("xhcid: failed to enable MSI");
        info!("Enabled MSI");

        (Some(interrupt_handle), InterruptMethod::Msi)
    } else if msix_enabled {
        let capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("xhcid: failed to retrieve the MSI-X capability structure from pcid") {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        let table_size = capability.table_size();
        let table_base = capability.table_base_pointer(pci_config.func.bars);
        let table_min_length = table_size * 16;
        let pba_min_length = crate::xhci::scheme::div_round_up(table_size, 8);

        let pba_base = capability.pba_base_pointer(pci_config.func.bars);

        if !(bar_ptr..bar_ptr + 65536).contains(&(table_base as u32 + table_min_length as u32)) {
            todo!()
        }
        if !(bar_ptr..bar_ptr + 65536).contains(&(pba_base as u32 + pba_min_length as u32)) {
            todo!()
        }

        let virt_table_base = ((table_base - bar_ptr as usize) + address) as *mut MsixTableEntry;
        let virt_pba_base = ((pba_base - bar_ptr as usize) + address) as *mut u64;

        let mut info = xhci::MsixInfo {
            virt_table_base: NonNull::new(virt_table_base).unwrap(),
            virt_pba_base: NonNull::new(virt_pba_base).unwrap(),
            capability,
        };

        // Allocate one msi vector.

        let method = {
            use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
            let lapic_id = u8::try_from(destination_id).expect("xhcid: CPU id couldn't fit inside u8");
            let rh = false;
            let dm = false;
            let addr = x86_64_msix::message_address(lapic_id, rh, dm, 0b00);

            let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id).expect("xhcid: failed to allocate interrupt vector").expect("xhcid: no interrupt vectors left");
            let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

            table_entry_pointer.addr_lo.write(addr);
            table_entry_pointer.addr_hi.write(0);
            table_entry_pointer.msg_data.write(msg_data);
            table_entry_pointer.vec_ctl.writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);

            (Some(interrupt_handle), InterruptMethod::MsiX(Mutex::new(info)))
        };

        pcid_handle.enable_feature(PciFeature::MsiX).expect("xhcid: failed to enable MSI-X");
        info!("Enabled MSI-X");

        method
    } else if pci_config.func.legacy_interrupt_pin.is_some() {
        // legacy INTx# interrupt pins.
        (Some(File::open(format!("irq:{}", irq)).expect("xhcid: failed to open legacy IRQ file")), InterruptMethod::Intx)
    } else {
        // no interrupts at all
        (None, InterruptMethod::Polling)
    };

    std::thread::sleep(std::time::Duration::from_millis(300));

    print!(
        "{}",
        format!(" + XHCI {} on: {} IRQ: {}\n", name, bar, irq)
    );

    let socket_fd = syscall::open(
        format!(":usb/{}", name),
        syscall::O_RDWR | syscall::O_CREAT,
    )
    .expect("xhcid: failed to create usb scheme");
    let socket = Arc::new(Mutex::new(unsafe {
        File::from_raw_fd(socket_fd as RawFd)
    }));

    let hci = Arc::new(Xhci::new(name, address, interrupt_method, pcid_handle).expect("xhcid: failed to allocate device"));
    xhci::start_irq_reactor(&hci, irq_file);
    futures::executor::block_on(hci.probe()).expect("xhcid: failed to probe");

    let mut event_queue =
        EventQueue::<()>::new().expect("xhcid: failed to create event queue");

    syscall::setrens(0, 0).expect("xhcid: failed to enter null namespace");

    let todo = Arc::new(Mutex::new(Vec::<Packet>::new()));
    let todo_futures = Arc::new(Mutex::new(Vec::<Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>>::new()));

    let socket_fd = socket.lock().unwrap().as_raw_fd();
    let socket_packet = socket.clone();
    event_queue
        .add(socket_fd, move |_| -> io::Result<Option<()>> {
            let mut socket = socket_packet.lock().unwrap();
            let mut todo = todo.lock().unwrap();

            loop {
                let mut packet = Packet::default();
                match socket.read(&mut packet) {
                    Ok(0) => break,
                    Ok(_) => (),
                    Err(err) => return Err(err),
                }

                let a = packet.a;
                hci.handle(&mut packet);
                if packet.a == (-EWOULDBLOCK) as usize {
                    packet.a = a;
                    todo.push(packet);
                } else {
                    socket.write(&packet)?;
                }
            }
            Ok(None)
        })
        .expect("xhcid: failed to catch events on scheme file");

    event_queue
        .trigger_all(Event { fd: 0, flags: 0 })
        .expect("xhcid: failed to trigger events");

    event_queue.run().expect("xhcid: failed to handle events");

    unsafe {
        let _ = syscall::physunmap(address);
    }
}
