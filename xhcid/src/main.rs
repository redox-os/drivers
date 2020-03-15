#[macro_use]
extern crate bitflags;
extern crate event;
extern crate plain;
extern crate syscall;

use pcid_interface::{PcidServerHandle, PciFeature, PciFeatureInfo};
use pcid_interface::MsixTableEntry;

use event::{Event, EventQueue};
use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Result, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use std::{env, io};
use syscall::data::Packet;
use syscall::error::EWOULDBLOCK;
use syscall::flag::{CloneFlags, PHYSMAP_NO_CACHE, PHYSMAP_WRITE};
use syscall::scheme::SchemeMut;

use crate::xhci::Xhci;

mod driver_interface;
mod usb;
mod xhci;

fn main() {
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

    if has_msi && !msi_enabled {
        pcid_handle.enable_feature(PciFeature::Msi).expect("xhcid: failed to enable MSI");
        msi_enabled = true;
        println!("Enabled MSI");
    }
    if has_msi && msi_enabled && has_msix && !msix_enabled {
        pcid_handle.enable_feature(PciFeature::MsiX).expect("xhcid: failed to enable MSI-X");
        msix_enabled = true;
        println!("Enabled MSI-X");
    }

    if msi_enabled && !msix_enabled {
        todo!("only msi-x is currently implemented")
    }
    if msix_enabled {
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

        let virt_table_base = ((table_base - bar_ptr as usize) + address) as *const MsixTableEntry;
        let virt_pba_base = ((pba_base - bar_ptr as usize) + address) as *const u64;

        for k in 0..table_size {
            assert_eq!(std::mem::size_of::<MsixTableEntry>(), 16);
            let table_entry_pointer = unsafe { virt_table_base.offset(k as isize).as_ref().unwrap() };
            let pba_pointer = unsafe { virt_pba_base.offset(k as isize / 64).as_ref().unwrap() };
            let pba_bit = k % 64;

            dbg!(table_entry_pointer, (*pba_pointer >> pba_bit) & 1);
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(300));

    // Daemonize
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    let mut args = env::args().skip(1);

    let mut name = args.next().expect("xhcid: no name provided");
    name.push_str("_xhci");

    print!(
        "{}",
        format!(" + XHCI {} on: {} IRQ: {}\n", name, bar, irq)
    );

    let socket_fd = syscall::open(
        format!(":usb/{}", name),
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK,
    )
    .expect("xhcid: failed to create usb scheme");
    let socket = Arc::new(RefCell::new(unsafe {
        File::from_raw_fd(socket_fd as RawFd)
    }));

    let mut irq_file =
        File::open(format!("irq:{}", irq)).expect("xhcid: failed to open IRQ file");

    {
        let hci = Arc::new(RefCell::new(
            Xhci::new(name, address).expect("xhcid: failed to allocate device"),
        ));

        hci.borrow_mut().probe().expect("xhcid: failed to probe");

        let mut event_queue =
            EventQueue::<()>::new().expect("xhcid: failed to create event queue");

        syscall::setrens(0, 0).expect("xhcid: failed to enter null namespace");

        let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

        let hci_irq = hci.clone();
        let socket_irq = socket.clone();
        let todo_irq = todo.clone();
        event_queue
            .add(irq_file.as_raw_fd(), move |_| -> Result<Option<()>> {
                let mut irq = [0; 8];
                irq_file.read(&mut irq)?;

                if hci_irq.borrow_mut().trigger_irq() {
                    irq_file.write(&mut irq)?;

                    let mut todo = todo_irq.borrow_mut();
                    let mut i = 0;
                    while i < todo.len() {
                        let a = todo[i].a;
                        hci_irq.borrow_mut().handle(&mut todo[i]);
                        if todo[i].a == (-EWOULDBLOCK) as usize {
                            todo[i].a = a;
                            i += 1;
                        } else {
                            socket_irq.borrow_mut().write(&mut todo[i])?;
                            todo.remove(i);
                        }
                    }
                }

                Ok(None)
            })
            .expect("xhcid: failed to catch events on IRQ file");

        let socket_fd = socket.borrow().as_raw_fd();
        let socket_packet = socket.clone();
        event_queue
            .add(socket_fd, move |_| -> Result<Option<()>> {
                loop {
                    let mut packet = Packet::default();
                    match socket_packet.borrow_mut().read(&mut packet) {
                        Ok(0) => break,
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        Ok(_) => (),
                        Err(err) => return Err(err),
                    }

                    let a = packet.a;
                    hci.borrow_mut().handle(&mut packet);
                    if packet.a == (-EWOULDBLOCK) as usize {
                        packet.a = a;
                        todo.borrow_mut().push(packet);
                    } else {
                        socket_packet.borrow_mut().write(&mut packet)?;
                    }
                }
                Ok(None)
            })
            .expect("xhcid: failed to catch events on scheme file");

        event_queue
            .trigger_all(Event { fd: 0, flags: 0 })
            .expect("xhcid: failed to trigger events");

        event_queue.run().expect("xhcid: failed to handle events");
    }
    unsafe {
        let _ = syscall::physunmap(address);
    }
}
