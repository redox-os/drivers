//#![deny(warnings)]

use event::{user_data, EventQueue};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::{iter, mem};

use common::io::{Io, Mmio, Pio};
use pcid_interface::PciFunctionHandle;

use common::dma::Dma;

use crate::bga::Bga;

mod bga;

const VBOX_REQUEST_HEADER_VERSION: u32 = 0x10001;
const VBOX_VMMDEV_VERSION: u32 = 0x00010003;

const VBOX_EVENT_DISPLAY: u32 = 1 << 2;
const VBOX_EVENT_MOUSE: u32 = 1 << 9;

/// VBox VMMDevMemory
#[repr(C, packed)]
struct VboxVmmDev {
    size: Mmio<u32>,
    version: Mmio<u32>,
    host_events: Mmio<u32>,
    guest_events: Mmio<u32>,
}

/// VBox Guest packet header
#[repr(C, packed)]
struct VboxHeader {
    /// Size of the entire packet (including this header)
    size: Mmio<u32>,
    /// Version; always VBOX_REQUEST_HEADER_VERSION
    version: Mmio<u32>,
    /// Request type
    request: Mmio<u32>,
    /// Return code
    result: Mmio<u32>,
    _reserved1: Mmio<u32>,
    _reserved2: Mmio<u32>,
}

/// VBox Get Mouse
#[repr(C, packed)]
struct VboxGetMouse {
    header: VboxHeader,
    features: Mmio<u32>,
    x: Mmio<u32>,
    y: Mmio<u32>,
}

impl VboxGetMouse {
    fn request() -> u32 {
        1
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Set Mouse
#[repr(C, packed)]
struct VboxSetMouse {
    header: VboxHeader,
    features: Mmio<u32>,
    x: Mmio<u32>,
    y: Mmio<u32>,
}

impl VboxSetMouse {
    fn request() -> u32 {
        2
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Acknowledge Events packet
#[repr(C, packed)]
struct VboxAckEvents {
    header: VboxHeader,
    events: Mmio<u32>,
}

impl VboxAckEvents {
    fn request() -> u32 {
        41
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Guest Capabilities packet
#[repr(C, packed)]
struct VboxGuestCaps {
    header: VboxHeader,
    caps: Mmio<u32>,
}

impl VboxGuestCaps {
    fn request() -> u32 {
        55
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/* VBox GetDisplayChange packet */
struct VboxDisplayChange {
    header: VboxHeader,
    xres: Mmio<u32>,
    yres: Mmio<u32>,
    bpp: Mmio<u32>,
    eventack: Mmio<u32>,
}

impl VboxDisplayChange {
    fn request() -> u32 {
        51
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Guest Info packet (legacy)
#[repr(C, packed)]
struct VboxGuestInfo {
    header: VboxHeader,
    version: Mmio<u32>,
    ostype: Mmio<u32>,
}

impl VboxGuestInfo {
    fn request() -> u32 {
        50
    }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

fn main() {
    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();

    let mut name = pci_config.func.name();
    name.push_str("_vbox");

    let bar0 = pci_config.func.bars[0].expect_port();

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("vboxd: no legacy interrupts supported");

    println!(" + VirtualBox {}", pci_config.func.display());

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        common::acquire_port_io_rights().expect("vboxd: failed to get I/O permission");

        let mut width = 0;
        let mut height = 0;
        let mut display_opt = File::open("inputd:producer").ok();
        if let Some(ref display) = display_opt {
            let mut buf: [u8; 4096] = [0; 4096];
            if let Ok(count) = libredox::call::fpath(display.as_raw_fd() as usize, &mut buf) {
                let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
                let res = path.split(":").nth(1).unwrap_or("");
                width = res
                    .split("/")
                    .nth(1)
                    .unwrap_or("")
                    .parse::<u32>()
                    .unwrap_or(0);
                height = res
                    .split("/")
                    .nth(2)
                    .unwrap_or("")
                    .parse::<u32>()
                    .unwrap_or(0);
            }
        }

        let mut irq_file = irq.irq_handle("vboxd");

        let mut port = Pio::<u32>::new(bar0 as u16);
        let address = unsafe { pcid_handle.map_bar(1) }.ptr.as_ptr();
        {
            let vmmdev = unsafe { &mut *(address as *mut VboxVmmDev) };

            let mut guest_info = VboxGuestInfo::new().expect("vboxd: failed to map GuestInfo");
            guest_info.version.write(VBOX_VMMDEV_VERSION);
            guest_info.ostype.write(0x100);
            port.write(guest_info.physical() as u32);

            let mut guest_caps = VboxGuestCaps::new().expect("vboxd: failed to map GuestCaps");
            guest_caps.caps.write(1 << 2);
            port.write(guest_caps.physical() as u32);

            let mut set_mouse = VboxSetMouse::new().expect("vboxd: failed to map SetMouse");
            set_mouse.features.write(1 << 4 | 1);
            port.write(set_mouse.physical() as u32);

            vmmdev
                .guest_events
                .write(VBOX_EVENT_DISPLAY | VBOX_EVENT_MOUSE);

            user_data! {
                enum Source {
                    Irq,
                }
            }

            let event_queue =
                EventQueue::<Source>::new().expect("vboxd: Could not create event queue.");
            event_queue
                .subscribe(
                    irq_file.as_raw_fd() as usize,
                    Source::Irq,
                    event::EventFlags::READ,
                )
                .unwrap();

            daemon.ready().expect("failed to signal readiness");

            libredox::call::setrens(0, 0).expect("vboxd: failed to enter null namespace");

            let mut bga = Bga::new();
            let get_mouse = VboxGetMouse::new().expect("vboxd: failed to map GetMouse");
            let display_change =
                VboxDisplayChange::new().expect("vboxd: failed to map DisplayChange");
            let ack_events = VboxAckEvents::new().expect("vboxd: failed to map AckEvents");

            for Source::Irq in iter::once(Source::Irq)
                .chain(event_queue.map(|e| e.expect("vboxd: failed to get next event").user_data))
            {
                let mut irq = [0; 8];
                if irq_file.read(&mut irq).unwrap() >= irq.len() {
                    let host_events = vmmdev.host_events.read();
                    if host_events != 0 {
                        port.write(ack_events.physical() as u32);
                        irq_file.write(&irq).unwrap();

                        if host_events & VBOX_EVENT_DISPLAY == VBOX_EVENT_DISPLAY {
                            port.write(display_change.physical() as u32);
                            if let Some(ref mut display) = display_opt {
                                let new_width = display_change.xres.read();
                                let new_height = display_change.yres.read();
                                if width != new_width || height != new_height {
                                    width = new_width;
                                    height = new_height;
                                    println!("Display {}, {}", width, height);
                                    bga.set_size(width as u16, height as u16);
                                    let _ = display.write(
                                        &orbclient::ResizeEvent { width, height }.to_event(),
                                    );
                                }
                            }
                        }

                        if host_events & VBOX_EVENT_MOUSE == VBOX_EVENT_MOUSE {
                            port.write(get_mouse.physical() as u32);
                            if let Some(ref mut display) = display_opt {
                                let x = get_mouse.x.read() * width / 0x10000;
                                let y = get_mouse.y.read() * height / 0x10000;
                                let _ = display.write(
                                    &orbclient::MouseEvent {
                                        x: x as i32,
                                        y: y as i32,
                                    }
                                    .to_event(),
                                );
                            }
                        }
                    }
                }
            }
        }

        std::process::exit(0);
    })
    .expect("vboxd: failed to daemonize");
}
