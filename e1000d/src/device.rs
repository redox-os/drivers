//! device driver for the Intel8254x ethernet device
use std::{cmp, mem, ptr, slice};

use netutils::setcfg;
use syscall::error::{Error, EACCES, EWOULDBLOCK, Result};
use syscall::flag::O_NONBLOCK;
use syscall::io::Dma;
use syscall::scheme::Scheme;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: one /// line describing each constant. Just defining the acronym
// would be very beneficial for all of these constants
const CTRL: u32 = 0x00;
const CTRL_LRST: u32 = 1 << 3;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_ILOS: u32 = 1 << 7;
const CTRL_VME: u32 = 1 << 30;
const CTRL_PHY_RST: u32 = 1 << 31;

// TODO: status bit for what? 
const STATUS: u32 = 0x08;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const FCAL: u32 = 0x28;
const FCAH: u32 = 0x2C;
const FCT: u32 = 0x30;
const FCTTV: u32 = 0x170;

// TODO: ICR means what?
const ICR: u32 = 0xC0;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const IMS: u32 = 0xD0;
const IMS_TXDW: u32 = 1;
const IMS_TXQE: u32 = 1 << 1;
const IMS_LSC: u32 = 1 << 2;
const IMS_RXSEQ: u32 = 1 << 3;
const IMS_RXDMT: u32 = 1 << 4;
const IMS_RX: u32 = 1 << 6;
const IMS_RXT: u32 = 1 << 7;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const RCTL: u32 = 0x100;
const RCTL_EN: u32 = 1 << 1;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_LPE: u32 = 1 << 5;
const RCTL_LBM: u32 = 1 << 6 | 1 << 7;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_BSIZE1: u32 = 1 << 16;
const RCTL_BSIZE2: u32 = 1 << 17;
const RCTL_BSEX: u32 = 1 << 25;
const RCTL_SECRC: u32 = 1 << 26;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const RDBAL: u32 = 0x2800;
const RDBAH: u32 = 0x2804;
const RDLEN: u32 = 0x2808;
const RDH: u32 = 0x2810;
const RDT: u32 = 0x2818;

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const RAL0: u32 = 0x5400;
const RAH0: u32 = 0x5404;

#[derive(Debug)]
#[repr(packed)]
// TODO: what does Rd stand for?
struct Rd {
    buffer: u64,
    length: u16,
    checksum: u16,
    status: u8,
    error: u8,
    special: u16,
}

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const RD_DD: u8 = 1;
const RD_EOP: u8 = 1 << 1;

const TCTL: u32 = 0x400;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;

const TDBAL: u32 = 0x3800;
const TDBAH: u32 = 0x3804;
const TDLEN: u32 = 0x3808;
const TDH: u32 = 0x3810;
const TDT: u32 = 0x3818;

#[derive(Debug)]
#[repr(packed)]
// TODO: what is a Td?
struct Td {
    buffer: u64,
    length: u16,
    cso: u8,
    command: u8,
    status: u8,
    css: u8,
    special: u16,
}

// TODO: One // line here to describe what this GROUP of constants is
// TODO: leave one line description of these using ///
const TD_CMD_EOP: u8 = 1;
const TD_CMD_IFCS: u8 = 1 << 1;
const TD_CMD_RS: u8 = 1 << 3;
const TD_DD: u8 = 1;

/// The Intel8254x device and it's associated device memory
/// 
/// TODO: are the values pointing to physical memory, memory on the device buffers... what?
pub struct Intel8254x {
    base: usize,
    receive_buffer: [Dma<[u8; 16384]>; 16],
    receive_ring: Dma<[Rd; 16]>,
    transmit_buffer: [Dma<[u8; 16384]>; 16],
    transmit_ring: Dma<[Td; 16]>
}

impl Scheme for Intel8254x {
    fn open(&self, _path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid == 0 {
            Ok(flags)
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn dup(&self, id: usize, _buf: &[u8]) -> Result<usize> {
        Ok(id)
    }

    /// TODO: what is the basic archetecture here? Are we reading data from physical
    /// memory on the device to RAM? What is happening here? How are we sending
    /// the data back to the caller?
    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        let head = unsafe { self.read(RDH) };
        let mut tail = unsafe { self.read(RDT) };

        tail += 1;
        if tail >= self.receive_ring.len() as u32 {
            tail = 0;
        }

        if tail != head {
            let rd = unsafe { &mut * (self.receive_ring.as_ptr().offset(tail as isize) as *mut Rd) };
            if rd.status & RD_DD == RD_DD {
                rd.status = 0;

                let data = &self.receive_buffer[tail as usize][.. rd.length as usize];

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    buf[i] = data[i];
                    i += 1;
                }

                unsafe { self.write(RDT, tail) };

                return Ok(i);
            }
        }

        if id & O_NONBLOCK == O_NONBLOCK {
            Ok(0)
        } else {
            Err(Error::new(EWOULDBLOCK))
        }
    }

    /// same questions as read -- where is the memory? What is the overall archetecture
    /// here?
    fn write(&self, _id: usize, buf: &[u8]) -> Result<usize> {
        loop {
            let head = unsafe { self.read(TDH) };
            let mut tail = unsafe { self.read(TDT) };
            let old_tail = tail;

            tail += 1;
            if tail >= self.transmit_ring.len() as u32 {
                tail = 0;
            }

            if tail != head {
                let td = unsafe { &mut * (self.transmit_ring.as_ptr().offset(old_tail as isize) as *mut Td) };

                td.cso = 0;
                td.command = TD_CMD_EOP | TD_CMD_IFCS | TD_CMD_RS;
                td.status = 0;
                td.css = 0;
                td.special = 0;

                td.length = (cmp::min(buf.len(), 0x3FFF)) as u16;

                let mut data = unsafe { slice::from_raw_parts_mut(self.transmit_buffer[old_tail as usize].as_ptr() as *mut u8, td.length as usize) };

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    data[i] = buf[i];
                    i += 1;
                }

                unsafe { self.write(TDT, tail) };

                while td.status == 0 {
                    unsafe { asm!("pause" : : : "memory" : "intel", "volatile"); }
                }

                return Ok(i);
            }

            unsafe { asm!("pause" : : : "memory" : "intel", "volatile"); }
        }
    }

    /// why is this always Ok?
    fn fevent(&self, _id: usize, _flags: usize) -> Result<usize> {
        Ok(0)
    }

    /// why is this always Ok?
    fn fsync(&self, _id: usize) -> Result<usize> {
        Ok(0)
    }

    /// why is this always Ok? Does close actually not do anything???
    fn close(&self, _id: usize) -> Result<usize> {
        Ok(0)
    }
}

impl Intel8254x {
    pub unsafe fn new(base: usize) -> Result<Self> {
        // Why this specific amount of Dma's (what is a Dma??).
        // Answering whether this stuff is on the device itself will help me
        // understand
        let mut module = Intel8254x {
            base: base,
            receive_buffer: [Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?],
            receive_ring: Dma::zeroed()?,
            transmit_buffer: [Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
                            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?],
            transmit_ring: Dma::zeroed()?
        };

        module.init();

        Ok(module)
    }

    /// I don't understand where the data is going. Is it getting stored in the receive
    /// buffer?
    pub unsafe fn irq(&self) -> bool {
        let icr = self.read(ICR);
        icr != 0
    }

    /// what is the difference between this and read? What are the use cases here
    pub fn next_read(&self) -> usize {
        let head = unsafe { self.read(RDH) };
        let mut tail = unsafe { self.read(RDT) };

        tail += 1;
        if tail >= self.receive_ring.len() as u32 {
            tail = 0;
        }

        if tail != head {
            let rd = unsafe { &* (self.receive_ring.as_ptr().offset(tail as isize) as *const Rd) };
            if rd.status & RD_DD == RD_DD {
                return rd.length as usize;
            }
        }

        0
    }

    // TODO: whoa whoa... what? this just returns... a pointer as a u32? Why is it u32 and not u64?
    // Is this pretty much just a shortcut to the data that exists on the device?
    pub unsafe fn read(&self, register: u32) -> u32 {
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    // TODO: similar questions to read here
    pub unsafe fn write(&self, register: u32, data: u32) -> u32 {
        ptr::write_volatile((self.base + register as usize) as *mut u32, data);
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    /// VERIFY sets or clears the requested flag to the register without changing other flags
    pub unsafe fn flag(&self, register: u32, flag: u32, value: bool) {
        if value {
            self.write(register, self.read(register) | flag);
        } else {
            self.write(register, self.read(register) & (0xFFFFFFFF - flag));
        }
    }

    /// Initialize the ethernet driver:
    /// - VERIFY: setting relevant cpu register options
    /// - VERIFY: setting the mac address
    /// - VERIFY: initializing the receive and transmit buffers and telling the
    ///   device where they are located (???)
    /// - VERIFY: setting up the device's registers and finally enabling it for operation
    pub unsafe fn init(&mut self) {
        // Enable auto negotiate, link, clear reset, do not Invert Loss-Of Signal
        self.flag(CTRL, CTRL_ASDE | CTRL_SLU, true);
        self.flag(CTRL, CTRL_LRST, false);
        self.flag(CTRL, CTRL_PHY_RST, false);
        self.flag(CTRL, CTRL_ILOS, false);

        // No flow control
        self.write(FCAH, 0);
        self.write(FCAL, 0);
        self.write(FCT, 0);
        self.write(FCTTV, 0);

        // Do not use VLANs
        self.flag(CTRL, CTRL_VME, false);

        // TODO: Clear statistical counters

        let mac_low = self.read(RAL0);
        let mac_high = self.read(RAH0);
        let mac = [mac_low as u8,
                    (mac_low >> 8) as u8,
                    (mac_low >> 16) as u8,
                    (mac_low >> 24) as u8,
                    mac_high as u8,
                    (mac_high >> 8) as u8];
        print!("{}", format!("   - MAC: {:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}\n", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]));
        let _ = setcfg("mac", &format!("{:>02X}.{:>02X}.{:>02X}.{:>02X}.{:>02X}.{:>02X}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]));

        //
        // MTA => 0;
        //

        // Initialize Receive Buffer
        for i in 0..self.receive_ring.len() {
            self.receive_ring[i].buffer = self.receive_buffer[i].physical() as u64;
        }

        self.write(RDBAH, (self.receive_ring.physical() >> 32) as u32);
        self.write(RDBAL, self.receive_ring.physical() as u32);
        self.write(RDLEN, (self.receive_ring.len() * mem::size_of::<Rd>()) as u32);
        self.write(RDH, 0);
        self.write(RDT, self.receive_ring.len() as u32 - 1);

        // Initialize Transmit Buffer
        for i in 0..self.transmit_ring.len() {
            self.transmit_ring[i].buffer = self.transmit_buffer[i].physical() as u64;
        }

        self.write(TDBAH, (self.transmit_ring.physical() >> 32) as u32);
        self.write(TDBAL, self.transmit_ring.physical() as u32);
        self.write(TDLEN, (self.transmit_ring.len() * mem::size_of::<Td>()) as u32);
        self.write(TDH, 0);
        self.write(TDT, 0);

        // TODO: what are we doing after this point???
        self.write(IMS, IMS_RXT | IMS_RX | IMS_RXDMT | IMS_RXSEQ); // | IMS_LSC | IMS_TXQE | IMS_TXDW

        self.flag(RCTL, RCTL_EN, true);
        self.flag(RCTL, RCTL_UPE, true);
        // self.flag(RCTL, RCTL_MPE, true);
        self.flag(RCTL, RCTL_LPE, true);
        self.flag(RCTL, RCTL_LBM, false);
        // RCTL.RDMTS = Minimum threshold size ???
        // RCTL.MO = Multicast offset
        self.flag(RCTL, RCTL_BAM, true);
        self.flag(RCTL, RCTL_BSIZE1, true);
        self.flag(RCTL, RCTL_BSIZE2, false);
        self.flag(RCTL, RCTL_BSEX, true);
        self.flag(RCTL, RCTL_SECRC, true);

        self.flag(TCTL, TCTL_EN, true);
        self.flag(TCTL, TCTL_PSP, true);
        // TCTL.CT = Collision threshold
        // TCTL.COLD = Collision distance
        // TIPG Packet Gap
        // TODO ...
    }
}
