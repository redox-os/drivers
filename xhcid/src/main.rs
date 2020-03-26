#[macro_use]
extern crate bitflags;
extern crate event;
extern crate plain;
extern crate syscall;

use pcid_interface::{PcidServerHandle, PciFeature, PciFeatureInfo};
use pcid_interface::msi::{MsiCapability, MsixCapability, MsixTableEntry};

use event::{Event, EventQueue};
use std::convert::TryInto;
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::env;
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::flag::{CloneFlags, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::scheme::SchemeMut;
use syscall::io::Io;

use crate::xhci::{InterruptMethod, Xhci};

mod driver_interface;
mod usb;
mod xhci;

/// Read the local APIC id of the bootstrap processor.
fn read_bsp_apic_id() -> io::Result<u32> {
    let mut buffer = [0u8; 8];

    let mut file = File::open("irq:bsp")?;
    let bytes_read = file.read(&mut buffer)?;

    Ok(if bytes_read == 8 {
        u64::from_le_bytes(buffer) as u32
    } else if bytes_read == 4 {
        u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]])
    } else {
        panic!("`irq:` scheme responded with {} bytes, expected {}", bytes_read, std::mem::size_of::<usize>());
    })
}
/// Allocate an interrupt vector, located at the BSP's IDT.
fn allocate_interrupt_vector() -> io::Result<Option<(u8, File)>> {
    let available_irqs = fs::read_dir("irq:")?;

    for entry in available_irqs {
        let entry = entry?;
        let path = entry.path();

        let file_name = match path.file_name() {
            Some(f) => f,
            None => continue,
        };

        let path_str = match file_name.to_str() {
            Some(s) => s,
            None => continue,
        };

        if let Ok(irq_number) = path_str.parse::<u8>() {
            // if found, reserve the irq
            let irq_handle = File::create(format!("irq:{}", irq_number))?;
            let interrupt_vector = irq_number + 32;
            return Ok(Some((interrupt_vector, irq_handle)));
        }
    }
    Ok(None)
}

async fn handle_packet(hci: Arc<Xhci>, packet: Packet) -> Packet {
    todo!()
}

fn main() {
    // Daemonize
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    let mut pcid_handle = PcidServerHandle::connect_default().expect("xhcid: failed to setup channel to pcid");
    let pci_config = pcid_handle.fetch_config().expect("xhcid: failed to fetch config");
    println!("XHCI PCI CONFIG: {:?}", pci_config);

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
    println!("XHCI PCI FEATURES: {:?}", all_pci_features);

    let (has_msi, mut msi_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msi(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));
    let (has_msix, mut msix_enabled) = all_pci_features.iter().map(|(feature, status)| (feature.is_msix(), status.is_enabled())).find(|&(f, _)| f).unwrap_or((false, false));

    dbg!(has_msi, msi_enabled);
    dbg!(has_msix, msix_enabled);

    if has_msi && !msi_enabled && !has_msix {
        pcid_handle.enable_feature(PciFeature::Msi).expect("xhcid: failed to enable MSI");
        println!("Enabled MSI");
        msi_enabled = true;
    }
    if has_msix && !msix_enabled {
        pcid_handle.enable_feature(PciFeature::MsiX).expect("xhcid: failed to enable MSI-X");
        println!("Enabled MSI-X");
        msix_enabled = true;
    }

    let (mut irq_file, interrupt_method) = if msi_enabled && !msix_enabled {
        let mut capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("xhcid: failed to retrieve the MSI capability structure from pcid") {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // use one vector
        capability.set_multi_message_enabled(0);

        todo!("msi (msix is implemented though)")
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
        dbg!(table_size, table_base, table_min_length, pba_base);

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

        {
            use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

            // primary interrupter
            let k = 0;

            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = info.table_entry_pointer(k);

            let destination_id = read_bsp_apic_id().expect("xhcid: failed to read BSP apic id");
            let rh = false;
            let dm = false;
            let addr = x86_64_msix::message_address(destination_id.try_into().expect("xhcid: BSP apic id couldn't fit u8"), rh, dm, 0b00);

            let (vector, interrupt_handle) = allocate_interrupt_vector().expect("xhcid: failed to allocate interrupt vector").expect("xhcid: no interrupt vectors left");
            let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

            table_entry_pointer.addr_lo.write(addr);
            table_entry_pointer.addr_hi.write(0);
            table_entry_pointer.msg_data.write(msg_data);
            table_entry_pointer.vec_ctl.writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);

            (Some(interrupt_handle), InterruptMethod::MsiX(info))
        }
    } else if pci_config.func.legacy_interrupt_pin.is_some() {
        // legacy INTx# interrupt pins.
        (Some(File::open(format!("irq:{}", irq)).expect("xhcid: failed to open legacy IRQ file")), InterruptMethod::Intx)
    } else {
        // no interrupts at all
        (None, InterruptMethod::Polling)
    };

    std::thread::sleep(std::time::Duration::from_millis(300));

    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

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

    let hci = Arc::new(Xhci::new(name, address, interrupt_method, irq_file.as_mut(), pcid_handle).expect("xhcid: failed to allocate device"));
    hci.probe().expect("xhcid: failed to probe");

    let mut event_queue =
        EventQueue::<()>::new().expect("xhcid: failed to create event queue");

    syscall::setrens(0, 0).expect("xhcid: failed to enter null namespace");

    let todo = Arc::new(Mutex::new(Vec::<Packet>::new()));
    let todo_futures = Arc::new(Mutex::new(Vec::<Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>>::new()));

    if let Some(irq_file) = irq_file {
        let hci_irq = hci.clone();
        let socket_irq = socket.clone();
        let todo_irq = todo.clone();
        event_queue
            .add(irq_file.as_raw_fd(), move |_| -> io::Result<Option<()>> {
                let mut irq = [0; 8];
                irq_file.read(&mut irq)?;

                let hci = hci_irq.lock().unwrap();
                let socket = socket_irq.lock().unwrap();
                let todo = todo_irq.lock().unwrap();

                if hci.received_irq() {
                    hci.on_irq();

                    irq_file.write(&mut irq)?;

                    let mut i = 0;
                    while i < todo.len() {
                        let a = todo[i].a;
                        hci.handle(&mut todo[i]);
                        if todo[i].a == (-EWOULDBLOCK) as usize {
                            todo[i].a = a;
                            i += 1;
                        } else {
                            socket.write(&todo[i])?;
                            todo.remove(i);
                        }
                    }
                }

                Ok(None)
            })
            .expect("xhcid: failed to catch events on IRQ file");
    }

    let socket_fd = socket.lock().unwrap().as_raw_fd();
    let socket_packet = socket.clone();
    event_queue
        .add(socket_fd, move |_| -> io::Result<Option<()>> {
            let mut socket = socket_packet.lock().unwrap();
            let mut hci = hci.lock().unwrap();
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
