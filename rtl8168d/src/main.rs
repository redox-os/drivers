extern crate event;
extern crate netutils;
extern crate syscall;

use std::cell::RefCell;
use std::convert::{TryFrom, TryInto};
use std::{env, process};
use std::fs::File;
use std::io::{ErrorKind, Read, Result, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::ptr::NonNull;
use std::sync::Arc;

use event::EventQueue;
use pcid_interface::{MsiSetFeatureInfo, PcidServerHandle, PciFeature, PciFeatureInfo, SetFeatureInfo, SubdriverArguments};
use pcid_interface::irq_helpers::{read_bsp_apic_id, allocate_single_interrupt_vector};
use pcid_interface::msi::{MsixCapability, MsixTableEntry};
use redox_log::{RedoxLogger, OutputBuilder};
use syscall::{EventFlags, Packet, SchemeBlockMut};
use syscall::io::Io;

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
    match OutputBuilder::in_redox_logging_scheme("usb", "host", "rtl8168.log") {
        Ok(b) => logger = logger.with_output(
            // TODO: Add a configuration file for this
            b.with_filter(log::LevelFilter::Info)
                .flush_on_newline(true)
                .build()
        ),
        Err(error) => eprintln!("Failed to create rtl8168.log: {}", error),
    }

    #[cfg(target_os = "redox")]
    match OutputBuilder::in_redox_logging_scheme("usb", "host", "rtl8168.ansi.log") {
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
    pub virt_pba_base: NonNull<u64>,
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
    pub unsafe fn pba_pointer_unchecked(&mut self, k: usize) -> &mut u64 {
        &mut *self.virt_pba_base.as_ptr().offset(k as isize)
    }
    pub fn pba_pointer(&mut self, k: usize) -> &mut u64 {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.pba_pointer_unchecked(k) }
    }
    pub fn pba(&mut self, k: usize) -> bool {
        let byte = k / 64;
        let bit = k % 64;
        *self.pba_pointer(byte) & (1 << bit) != 0
    }
}

#[cfg(target_arch = "x86_64")]
fn get_int_method(pcid_handle: &mut PcidServerHandle) -> Option<File> {
    let pci_config = pcid_handle.fetch_config().expect("rtl8168d: failed to fetch config");

    let irq = pci_config.func.legacy_interrupt_line;

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
        use pcid_interface::msi::x86_64::{DeliveryMode, self as x86_64_msix};

        let capability = match pcid_handle.feature_info(PciFeature::Msi).expect("rtl8168d: failed to retrieve the MSI capability structure from pcid") {
            PciFeatureInfo::Msi(s) => s,
            PciFeatureInfo::MsiX(_) => panic!(),
        };
        // TODO: Allow allocation of up to 32 vectors.

        // TODO: Find a way to abstract this away, potantially as a helper module for
        // pcid_interface, so that this can be shared between nvmed, xhcid, ixgebd, etc..

        let destination_id = read_bsp_apic_id().expect("rtl8168d: failed to read BSP apic id");
        let lapic_id = u8::try_from(destination_id).expect("CPU id didn't fit inside u8");
        let msg_addr = x86_64_msix::message_address(lapic_id, false, false);

        let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id).expect("rtl8168d: failed to allocate interrupt vector").expect("rtl8168d: no interrupt vectors left");
        let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

        let set_feature_info = MsiSetFeatureInfo {
            multi_message_enable: Some(0),
            message_address: Some(msg_addr),
            message_upper_address: Some(0),
            message_data: Some(msg_data as u16),
            mask_bits: None,
        };
        pcid_handle.set_feature_info(SetFeatureInfo::Msi(set_feature_info)).expect("rtl8168d: failed to set feature info");

        pcid_handle.enable_feature(PciFeature::Msi).expect("rtl8168d: failed to enable MSI");
        log::info!("Enabled MSI");

        Some(interrupt_handle)
    } else if msix_enabled {
        let capability = match pcid_handle.feature_info(PciFeature::MsiX).expect("rtl8168d: failed to retrieve the MSI-X capability structure from pcid") {
            PciFeatureInfo::Msi(_) => panic!(),
            PciFeatureInfo::MsiX(s) => s,
        };
        let table_size = capability.table_size();
        let table_base = capability.table_base_pointer(pci_config.func.bars);
        let table_min_length = table_size * 16;
        let pba_min_length = div_round_up(table_size, 8);

        let pba_base = capability.pba_base_pointer(pci_config.func.bars);

        let bir = capability.table_bir() as usize;
        let (bar_ptr, bar_size) = pci_config.func.bars[bir].expect_mem();

        let address = unsafe {
            common::physmap(bar_ptr, bar_size, common::Prot::RW, common::MemoryType::Uncacheable)
                .expect("rtl8168d: failed to map address") as usize
        };

        if !(bar_ptr as u64..bar_ptr as u64 + bar_size as u64).contains(&(table_base as u64 + table_min_length as u64)) {
            panic!("Table {:#x}{:#x} outside of BAR {:#x}:{:#x}", table_base, table_base + table_min_length as usize, bar_ptr, bar_ptr + bar_size);
        }

        if !(bar_ptr as u64..bar_ptr as u64 + bar_size as u64).contains(&(pba_base as u64 + pba_min_length as u64)) {
            panic!("PBA {:#x}{:#x} outside of BAR {:#x}:{:#X}", pba_base, pba_base + pba_min_length as usize, bar_ptr, bar_ptr + bar_size);
        }

        let virt_table_base = ((table_base - bar_ptr) + address) as *mut MsixTableEntry;
        let virt_pba_base = ((pba_base - bar_ptr) + address) as *mut u64;

        let mut info = MsixInfo {
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

            let destination_id = read_bsp_apic_id().expect("rtl8168d: failed to read BSP apic id");
            let lapic_id = u8::try_from(destination_id).expect("rtl8168d: CPU id couldn't fit inside u8");
            let rh = false;
            let dm = false;
            let addr = x86_64_msix::message_address(lapic_id, rh, dm);

            let (vector, interrupt_handle) = allocate_single_interrupt_vector(destination_id).expect("rtl8168d: failed to allocate interrupt vector").expect("rtl8168d: no interrupt vectors left");
            let msg_data = x86_64_msix::message_data_edge_triggered(DeliveryMode::Fixed, vector);

            table_entry_pointer.addr_lo.write(addr);
            table_entry_pointer.addr_hi.write(0);
            table_entry_pointer.msg_data.write(msg_data);
            table_entry_pointer.vec_ctl.writef(MsixTableEntry::VEC_CTL_MASK_BIT, false);

            Some(interrupt_handle)
        };

        pcid_handle.enable_feature(PciFeature::MsiX).expect("rtl8168d: failed to enable MSI-X");
        log::info!("Enabled MSI-X");

        method
    } else if pci_config.func.legacy_interrupt_pin.is_some() {
        // legacy INTx# interrupt pins.
        Some(File::open(format!("irq:{}", irq)).expect("rtl8168d: failed to open legacy IRQ file"))
    } else {
        // no interrupts at all
        None
    }
}

//TODO: MSI on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
fn get_int_method(pcid_handle: &mut PcidServerHandle) -> Option<File> {
    let pci_config = pcid_handle.fetch_config().expect("rtl8168d: failed to fetch config");
    let irq = pci_config.func.legacy_interrupt_line;

    if pci_config.func.legacy_interrupt_pin.is_some() {
        // legacy INTx# interrupt pins.
        Some(File::open(format!("irq:{}", irq)).expect("rtl8168d: failed to open legacy IRQ file"))
    } else {
        // no interrupts at all
        None
    }
}

fn handle_update(
    socket: &mut File,
    device: &mut device::Rtl8168,
    todo: &mut Vec<Packet>,
) -> Result<bool> {
    // Handle any blocked packets
    let mut i = 0;
    while i < todo.len() {
        if let Some(a) = device.handle(&todo[i]) {
            let mut packet = todo.remove(i);
            packet.a = a;
            socket.write(&packet)?;
        } else {
            i += 1;
        }
    }

    // Check that the socket is empty
    loop {
        let mut packet = Packet::default();
        match socket.read(&mut packet) {
            Ok(0) => return Ok(true),
            Ok(_) => (),
            Err(err) => {
                if err.kind() == ErrorKind::WouldBlock {
                    break;
                } else {
                    return Err(err);
                }
            }
        }

        if let Some(a) = device.handle(&packet) {
            packet.a = a;
            socket.write(&packet)?;
        } else {
            todo.push(packet);
        }
    }

    Ok(false)
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
    log::info!(" + RTL8168 {} on: {:#X} size: {}", name, bar_ptr, bar_size);

    let address = unsafe {
        common::physmap(bar_ptr, bar_size, common::Prot::RW, common::MemoryType::Uncacheable)
            .expect("rtl8168d: failed to map address") as usize
    };

    let socket_fd = syscall::open(
        ":network",
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_NONBLOCK,
    )
    .expect("rtl8168d: failed to create network scheme");
    let socket = Arc::new(RefCell::new(unsafe {
        File::from_raw_fd(socket_fd as RawFd)
    }));

    //TODO: MSI-X
    let mut irq_file = get_int_method(&mut pcid_handle).expect("rtl8168d: no interrupt file");

    {
        let device = Arc::new(RefCell::new(unsafe {
            device::Rtl8168::new(address).expect("rtl8168d: failed to allocate device")
        }));

        let mut event_queue =
            EventQueue::<usize>::new().expect("rtl8168d: failed to create event queue");

        syscall::setrens(0, 0).expect("rtl8168d: failed to enter null namespace");

        daemon.ready().expect("rtl8168d: failed to mark daemon as ready");

        let todo = Arc::new(RefCell::new(Vec::<Packet>::new()));

        let device_irq = device.clone();
        let socket_irq = socket.clone();
        let todo_irq = todo.clone();
        event_queue
            .add(
                irq_file.as_raw_fd(),
                move |_event| -> Result<Option<usize>> {
                    let mut irq = [0; 8];
                    irq_file.read(&mut irq)?;
                    //TODO: This may be causing spurious interrupts
                    if unsafe { device_irq.borrow_mut().irq() } {
                        irq_file.write(&mut irq)?;

                        if handle_update(
                            &mut socket_irq.borrow_mut(),
                            &mut device_irq.borrow_mut(),
                            &mut todo_irq.borrow_mut(),
                        )? {
                            return Ok(Some(0));
                        }

                        let next_read = device_irq.borrow().next_read();
                        if next_read > 0 {
                            return Ok(Some(next_read));
                        }
                    }
                    Ok(None)
                },
            )
            .expect("rtl8168d: failed to catch events on IRQ file");

        let device_packet = device.clone();
        let socket_packet = socket.clone();
        event_queue
            .add(socket_fd as RawFd, move |_event| -> Result<Option<usize>> {
                if handle_update(
                    &mut socket_packet.borrow_mut(),
                    &mut device_packet.borrow_mut(),
                    &mut todo.borrow_mut(),
                )? {
                    return Ok(Some(0));
                }

                let next_read = device_packet.borrow().next_read();
                if next_read > 0 {
                    return Ok(Some(next_read));
                }

                Ok(None)
            })
            .expect("rtl8168d: failed to catch events on scheme file");

        let send_events = |event_count| {
            for (handle_id, _handle) in device.borrow().handles.iter() {
                socket
                    .borrow_mut()
                    .write(&Packet {
                        id: 0,
                        pid: 0,
                        uid: 0,
                        gid: 0,
                        a: syscall::number::SYS_FEVENT,
                        b: *handle_id,
                        c: syscall::flag::EVENT_READ.bits(),
                        d: event_count,
                    })
                    .expect("rtl8168d: failed to write event");
            }
        };

        for event_count in event_queue
            .trigger_all(event::Event { fd: 0, flags: EventFlags::empty() })
            .expect("rtl8168d: failed to trigger events")
        {
            send_events(event_count);
        }

        loop {
            let event_count = event_queue.run().expect("rtl8168d: failed to handle events");
            if event_count == 0 {
                //TODO: Handle todo
                break;
            }
            send_events(event_count);
        }
    }
    process::exit(0);
}

fn main() {
    redox_daemon::Daemon::new(daemon).expect("rtl8168d: failed to create daemon");
}
