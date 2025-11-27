use std::convert::TryInto;
use std::{cmp, mem, ptr, slice, thread, time};

use driver_network::NetworkAdapter;

use syscall::error::Result;

use common::dma::Dma;

const CTRL: u32 = 0x00;
const CTRL_LRST: u32 = 1 << 3;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_ILOS: u32 = 1 << 7;
const CTRL_RST: u32 = 1 << 26;
const CTRL_VME: u32 = 1 << 30;
const CTRL_PHY_RST: u32 = 1 << 31;

const STATUS: u32 = 0x08;

const FCAL: u32 = 0x28;
const FCAH: u32 = 0x2C;
const FCT: u32 = 0x30;
const FCTTV: u32 = 0x170;

const ICR: u32 = 0xC0;

const IMS: u32 = 0xD0;
const IMS_TXDW: u32 = 1;
const IMS_TXQE: u32 = 1 << 1;
const IMS_LSC: u32 = 1 << 2;
const IMS_RXSEQ: u32 = 1 << 3;
const IMS_RXDMT: u32 = 1 << 4;
const IMS_RX: u32 = 1 << 6;
const IMS_RXT: u32 = 1 << 7;

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

const RDBAL: u32 = 0x2800;
const RDBAH: u32 = 0x2804;
const RDLEN: u32 = 0x2808;
const RDH: u32 = 0x2810;
const RDT: u32 = 0x2818;

const RAL0: u32 = 0x5400;
const RAH0: u32 = 0x5404;

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
struct Rd {
    buffer: u64,
    length: u16,
    checksum: u16,
    status: u8,
    error: u8,
    special: u16,
}
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

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
struct Td {
    buffer: u64,
    length: u16,
    cso: u8,
    command: u8,
    status: u8,
    css: u8,
    special: u16,
}
const TD_CMD_EOP: u8 = 1;
const TD_CMD_IFCS: u8 = 1 << 1;
const TD_CMD_RS: u8 = 1 << 3;
const TD_DD: u8 = 1;

pub struct Intel8254x {
    base: usize,
    mac_address: [u8; 6],
    receive_buffer: [Dma<[u8; 16384]>; 16],
    receive_ring: Dma<[Rd; 16]>,
    receive_index: usize,
    transmit_buffer: [Dma<[u8; 16384]>; 16],
    transmit_ring: Dma<[Td; 16]>,
    transmit_ring_free: usize,
    transmit_index: usize,
    transmit_clean_index: usize,
}

#[derive(Copy, Clone)]
pub enum Handle {
    Data { flags: usize },
    Mac { offset: usize },
}

fn wrap_ring(index: usize, ring_size: usize) -> usize {
    (index + 1) & (ring_size - 1)
}

impl NetworkAdapter for Intel8254x {
    fn mac_address(&mut self) -> [u8; 6] {
        self.mac_address
    }

    fn available_for_read(&mut self) -> usize {
        let desc = unsafe { &*(self.receive_ring.as_ptr().add(self.receive_index) as *const Rd) };

        if desc.status & RD_DD == RD_DD {
            return desc.length as usize;
        }

        0
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        let desc = unsafe { &mut *(self.receive_ring.as_ptr().add(self.receive_index) as *mut Rd) };

        if desc.status & RD_DD == RD_DD {
            desc.status = 0;

            let data = &self.receive_buffer[self.receive_index][..desc.length as usize];

            let i = cmp::min(buf.len(), data.len());
            buf[..i].copy_from_slice(&data[..i]);

            unsafe { self.write_reg(RDT, self.receive_index as u32) };
            self.receive_index = wrap_ring(self.receive_index, self.receive_ring.len());

            return Ok(Some(i));
        }

        Ok(None)
    }

    fn write_packet(&mut self, buf: &[u8]) -> Result<usize> {
        if self.transmit_ring_free == 0 {
            loop {
                let desc = unsafe {
                    &*(self.transmit_ring.as_ptr().add(self.transmit_clean_index) as *const Td)
                };

                if desc.status != 0 {
                    self.transmit_clean_index =
                        wrap_ring(self.transmit_clean_index, self.transmit_ring.len());
                    self.transmit_ring_free += 1;
                } else if self.transmit_ring_free > 0 {
                    break;
                }

                if self.transmit_ring_free >= self.transmit_ring.len() {
                    break;
                }
            }
        }

        let desc =
            unsafe { &mut *(self.transmit_ring.as_ptr().add(self.transmit_index) as *mut Td) };

        let data = unsafe {
            slice::from_raw_parts_mut(
                self.transmit_buffer[self.transmit_index].as_ptr() as *mut u8,
                cmp::min(buf.len(), self.transmit_buffer[self.transmit_index].len()) as usize,
            )
        };

        let i = cmp::min(buf.len(), data.len());
        data[..i].copy_from_slice(&buf[..i]);

        desc.cso = 0;
        desc.command = TD_CMD_EOP | TD_CMD_IFCS | TD_CMD_RS;
        desc.status = 0;
        desc.css = 0;
        desc.special = 0;

        desc.length = (cmp::min(
            buf.len(),
            self.transmit_buffer[self.transmit_index].len() - 1,
        )) as u16;

        self.transmit_index = wrap_ring(self.transmit_index, self.transmit_ring.len());
        self.transmit_ring_free -= 1;

        unsafe { self.write_reg(TDT, self.transmit_index as u32) };

        Ok(i)
    }
}

fn dma_array<T, const N: usize>() -> Result<[Dma<T>; N]> {
    Ok((0..N)
        .map(|_| Ok(unsafe { Dma::zeroed()?.assume_init() }))
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .unwrap_or_else(|_| unreachable!()))
}
impl Intel8254x {
    pub unsafe fn new(base: usize) -> Result<Self> {
        #[rustfmt::skip]
        let mut module = Intel8254x {
            base,
            mac_address: [0; 6],
            receive_buffer: dma_array()?,
            receive_ring: Dma::zeroed()?.assume_init(),
            transmit_buffer: dma_array()?,
            receive_index: 0,
            transmit_ring: Dma::zeroed()?.assume_init(),
            transmit_ring_free: 16,
            transmit_index: 0,
            transmit_clean_index: 0,
        };

        module.init();

        Ok(module)
    }

    pub unsafe fn irq(&self) -> bool {
        let icr = self.read_reg(ICR);
        icr != 0
    }

    pub unsafe fn read_reg(&self, register: u32) -> u32 {
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    pub unsafe fn write_reg(&self, register: u32, data: u32) -> u32 {
        ptr::write_volatile((self.base + register as usize) as *mut u32, data);
        ptr::read_volatile((self.base + register as usize) as *mut u32)
    }

    pub unsafe fn flag(&self, register: u32, flag: u32, value: bool) {
        if value {
            self.write_reg(register, self.read_reg(register) | flag);
        } else {
            self.write_reg(register, self.read_reg(register) & !flag);
        }
    }

    pub unsafe fn init(&mut self) {
        self.flag(CTRL, CTRL_RST, true);
        while self.read_reg(CTRL) & CTRL_RST == CTRL_RST {
            log::trace!("Waiting for reset: {:X}", self.read_reg(CTRL));
        }

        // Enable auto negotiate, link, clear reset, do not Invert Loss-Of Signal
        self.flag(CTRL, CTRL_ASDE | CTRL_SLU, true);
        self.flag(CTRL, CTRL_LRST | CTRL_PHY_RST | CTRL_ILOS, false);

        // No flow control
        self.write_reg(FCAH, 0);
        self.write_reg(FCAL, 0);
        self.write_reg(FCT, 0);
        self.write_reg(FCTTV, 0);

        // Do not use VLANs
        self.flag(CTRL, CTRL_VME, false);

        // TODO: Clear statistical counters

        let mac_low = self.read_reg(RAL0);
        let mac_high = self.read_reg(RAH0);
        let mac = [
            mac_low as u8,
            (mac_low >> 8) as u8,
            (mac_low >> 16) as u8,
            (mac_low >> 24) as u8,
            mac_high as u8,
            (mac_high >> 8) as u8,
        ];
        log::debug!(
            "MAC: {:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
        self.mac_address = mac;

        //
        // MTA => 0;
        //

        // Receive Buffer
        for i in 0..self.receive_ring.len() {
            self.receive_ring[i].buffer = self.receive_buffer[i].physical() as u64;
        }

        self.write_reg(RDBAH, ((self.receive_ring.physical() as u64) >> 32) as u32);
        self.write_reg(RDBAL, self.receive_ring.physical() as u32);
        self.write_reg(
            RDLEN,
            (self.receive_ring.len() * mem::size_of::<Rd>()) as u32,
        );
        self.write_reg(RDH, 0);
        self.write_reg(RDT, self.receive_ring.len() as u32 - 1);

        // Transmit Buffer
        for i in 0..self.transmit_ring.len() {
            self.transmit_ring[i].buffer = self.transmit_buffer[i].physical() as u64;
        }

        self.write_reg(TDBAH, ((self.transmit_ring.physical() as u64) >> 32) as u32);
        self.write_reg(TDBAL, self.transmit_ring.physical() as u32);
        self.write_reg(
            TDLEN,
            (self.transmit_ring.len() * mem::size_of::<Td>()) as u32,
        );
        self.write_reg(TDH, 0);
        self.write_reg(TDT, 0);

        self.write_reg(IMS, IMS_RXT | IMS_RX | IMS_RXDMT | IMS_RXSEQ); // | IMS_LSC | IMS_TXQE | IMS_TXDW

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

        log::debug!("Waiting for link up: {:X}", self.read_reg(STATUS));
        while self.read_reg(STATUS) & 2 != 2 {
            thread::sleep(time::Duration::from_millis(100));
        }
        log::debug!(
            "Link is up with speed {}",
            match (self.read_reg(STATUS) >> 6) & 0b11 {
                0b00 => "10 Mb/s",
                0b01 => "100 Mb/s",
                _ => "1000 Mb/s",
            }
        );
    }
}
