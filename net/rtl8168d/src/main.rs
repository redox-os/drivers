use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;

use driver_network::NetworkScheme;
use event::{user_data, EventQueue};
use pcid_interface::irq_helpers::pci_allocate_interrupt_vector;
use pcid_interface::PciFunctionHandle;

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

fn map_bar(pcid_handle: &mut PciFunctionHandle) -> *mut u8 {
    let config = pcid_handle.config();

    // RTL8168 uses BAR2, RTL8169 uses BAR1, search in that order
    for &barnum in &[2, 1] {
        match config.func.bars[usize::from(barnum)] {
            pcid_interface::PciBar::Memory32 { .. } | pcid_interface::PciBar::Memory64 { .. } => unsafe {
                return pcid_handle.map_bar(barnum).ptr.as_ptr();
            },
            other => log::warn!("BAR {} is {:?} instead of memory BAR", barnum, other),
        }
    }
    panic!("rtl8168d: failed to find BAR");
}

fn daemon(daemon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PciFunctionHandle::connect_default();

    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_rtl8168");

    common::setup_logging(
        "net",
        "pci",
        &name,
        common::output_level(),
        common::file_level(),
    );

    log::info!("RTL8168 {}", pci_config.func.display());

    let bar = map_bar(&mut pcid_handle);

    let mut irq_file = pci_allocate_interrupt_vector(&mut pcid_handle, "rtl8168d");

    let mut scheme = NetworkScheme::new(
        move || unsafe {
            device::Rtl8168::new(bar as usize).expect("rtl8168d: failed to allocate device")
        },
        daemon,
        format!("network.{name}"),
    );

    user_data! {
        enum Source {
            Irq,
            Scheme,
        }
    }

    let event_queue = EventQueue::<Source>::new().expect("rtl8168d: Could not create event queue.");
    event_queue
        .subscribe(
            irq_file.irq_handle().as_raw_fd() as usize,
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

    libredox::call::setrens(0, 0).expect("rtl8168d: failed to enter null namespace");

    scheme.tick().unwrap();

    for event in event_queue.map(|e| e.expect("rtl8168d: failed to get next event")) {
        match event.user_data {
            Source::Irq => {
                let mut irq = [0; 8];
                irq_file.irq_handle().read(&mut irq).unwrap();
                //TODO: This may be causing spurious interrupts
                if unsafe { scheme.adapter_mut().irq() } {
                    irq_file.irq_handle().write(&mut irq).unwrap();

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
