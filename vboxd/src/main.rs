//#![deny(warnings)]

extern crate event;
extern crate orbclient;
extern crate syscall;

use event::EventQueue;
use std::mem;
use std::os::unix::io::AsRawFd;
use std::fs::File;
use std::io::{Result, Read, Write};

use pcid_interface::{PciBar, PcidServerHandle};
use syscall::call::iopl;
use syscall::flag::EventFlags;
use syscall::io::{Io, Mmio, Pio};

use common::dma::Dma;

use crate::bga::Bga;

mod bga;

const VBOX_REQUEST_HEADER_VERSION: u32 = 0x10001;
const VBOX_VMMDEV_VERSION: u32 = 0x00010003;

const VBOX_EVENT_DISPLAY: u32 = 1 << 2;
const VBOX_EVENT_MOUSE: u32 = 1 << 9;

/// VBox VMMDevMemory
#[repr(packed)]
struct VboxVmmDev {
    size: Mmio<u32>,
    version: Mmio<u32>,
    host_events: Mmio<u32>,
    guest_events: Mmio<u32>,
}

/// VBox Guest packet header
#[repr(packed)]
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
#[repr(packed)]
struct VboxGetMouse {
    header: VboxHeader,
    features: Mmio<u32>,
    x: Mmio<u32>,
    y: Mmio<u32>,
}

impl VboxGetMouse {
    fn request() -> u32 { 1 }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Set Mouse
#[repr(packed)]
struct VboxSetMouse {
    header: VboxHeader,
    features: Mmio<u32>,
    x: Mmio<u32>,
    y: Mmio<u32>,
}

impl VboxSetMouse {
    fn request() -> u32 { 2 }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Acknowledge Events packet
#[repr(packed)]
struct VboxAckEvents {
    header: VboxHeader,
    events: Mmio<u32>,
}

impl VboxAckEvents {
    fn request() -> u32 { 41 }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Guest Capabilities packet
#[repr(packed)]
struct VboxGuestCaps {
    header: VboxHeader,
    caps: Mmio<u32>,
}

impl VboxGuestCaps {
    fn request() -> u32 { 55 }

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
    fn request() -> u32 { 51 }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

/// VBox Guest Info packet (legacy)
#[repr(packed)]
struct VboxGuestInfo {
    header: VboxHeader,
    version: Mmio<u32>,
    ostype: Mmio<u32>,
}

impl VboxGuestInfo {
    fn request() -> u32 { 50 }

    fn new() -> syscall::Result<Dma<Self>> {
        let mut packet = unsafe { Dma::<Self>::zeroed()?.assume_init() };

        packet.header.size.write(mem::size_of::<Self>() as u32);
        packet.header.version.write(VBOX_REQUEST_HEADER_VERSION);
        packet.header.request.write(Self::request());

        Ok(packet)
    }
}

fn main() {
    let mut pcid_handle =
        PcidServerHandle::connect_default().expect("vboxd: failed to setup channel to pcid");
    let pci_config = pcid_handle
        .fetch_config()
        .expect("vboxd: failed to fetch config");

    let mut name = pci_config.func.name();
    name.push_str("_vbox");

    let bar0 = match pci_config.func.bars[0] {
        PciBar::Port(port) => port,
        _ => unreachable!(),
    };

    let bar1 = match pci_config.func.bars[1] {
        PciBar::Memory32(addr) => addr as usize,
        PciBar::Memory64(addr) => addr as usize,
        PciBar::None | PciBar::Port(_) => unreachable!(),
    };

    let irq = pci_config.func.legacy_interrupt_line;

    print!("{}", format!(" + VirtualBox {} on: {:X}, {:X}, IRQ {}\n", name, bar0, bar1, irq));

    // Daemonize
    redox_daemon::Daemon::new(move |daemon| {
        daemon.ready().expect("failed to signal readiness");

        unsafe { iopl(3).expect("vboxd: failed to get I/O permission"); };

        let mut width = 0;
        let mut height = 0;
        let mut display_opt = File::open("inputd:producer").ok();
        if let Some(ref display) = display_opt {
            let mut buf: [u8; 4096] = [0; 4096];
            if let Ok(count) = syscall::fpath(display.as_raw_fd() as usize, &mut buf) {
                let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
                let res = path.split(":").nth(1).unwrap_or("");
                width = res.split("/").nth(1).unwrap_or("").parse::<u32>().unwrap_or(0);
                height = res.split("/").nth(2).unwrap_or("").parse::<u32>().unwrap_or(0);
            }
        }

        let mut irq_file = File::open(format!("irq:{}", irq)).expect("vboxd: failed to open IRQ file");

        let mut port = Pio::<u32>::new(bar0 as u16);
        let address = unsafe { common::physmap(bar1, 4096, common::Prot::RW, common::MemoryType::Uncacheable).expect("vboxd: failed to map address") };
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

            vmmdev.guest_events.write(VBOX_EVENT_DISPLAY | VBOX_EVENT_MOUSE);

            let mut event_queue = EventQueue::<()>::new().expect("vboxd: failed to create event queue");

            syscall::setrens(0, 0).expect("vboxd: failed to enter null namespace");

            let mut bga = Bga::new();
            let get_mouse = VboxGetMouse::new().expect("vboxd: failed to map GetMouse");
            let display_change = VboxDisplayChange::new().expect("vboxd: failed to map DisplayChange");
            let ack_events = VboxAckEvents::new().expect("vboxd: failed to map AckEvents");
            event_queue.add(irq_file.as_raw_fd(), move |_event| -> Result<Option<()>> {
                let mut irq = [0; 8];
                if irq_file.read(&mut irq)? >= irq.len() {
                    let host_events = vmmdev.host_events.read();
                    if host_events != 0 {
                        port.write(ack_events.physical() as u32);
                        irq_file.write(&irq)?;

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
                                    let _ = display.write(&orbclient::ResizeEvent {
                                        width: width,
                                        height: height,
                                    }.to_event());
                                }
                            }
                        }

                        if host_events & VBOX_EVENT_MOUSE == VBOX_EVENT_MOUSE {
                            port.write(get_mouse.physical() as u32);
                            if let Some(ref mut display) = display_opt {
                                let x = get_mouse.x.read() * width / 0x10000;
                                let y = get_mouse.y.read() * height / 0x10000;
                                let _ = display.write(&orbclient::MouseEvent {
                                    x: x as i32,
                                    y: y as i32,
                                }.to_event());
                            }
                        }
                    }
                }
                Ok(None)
            }).expect("vboxd: failed to poll irq");

            event_queue.trigger_all(event::Event {
                fd: 0,
                flags: EventFlags::empty()
            }).expect("vboxd: failed to trigger events");

            event_queue.run().expect("vboxd: failed to run event loop");
        }

        std::process::exit(0);
    }).expect("vboxd: failed to daemonize");
}
