use std::convert::TryInto;
use std::mem;

use common::dma::Dma;
use common::io::{Io, Mmio, ReadOnly};
use common::timeout::Timeout;
use driver_network::NetworkAdapter;
use syscall::error::{Error, Result, EIO, EMSGSIZE};

#[repr(C, packed)]
struct Regs {
    mac: [Mmio<u32>; 2],
    _mar: [Mmio<u32>; 2],
    _dtccr: [Mmio<u32>; 2],
    _rsv0: [Mmio<u32>; 2],
    tnpds: [Mmio<u32>; 2],
    thpds: [Mmio<u32>; 2],
    _rsv1: [Mmio<u8>; 7],
    cmd: Mmio<u8>,
    tppoll: Mmio<u8>,
    _rsv2: [Mmio<u8>; 3],
    imr: Mmio<u16>,
    isr: Mmio<u16>,
    tcr: Mmio<u32>,
    rcr: Mmio<u32>,
    _tctr: Mmio<u32>,
    _rsv3: Mmio<u32>,
    cmd_9346: Mmio<u8>,
    _config: [Mmio<u8>; 6],
    _rsv4: Mmio<u8>,
    timer_int: Mmio<u32>,
    _rsv5: Mmio<u32>,
    _phys_ar: Mmio<u32>,
    _rsv6: [Mmio<u32>; 2],
    phys_sts: ReadOnly<Mmio<u8>>,
    _rsv7: [Mmio<u8>; 23],
    _wakeup: [Mmio<u32>; 16],
    _crc: [Mmio<u16>; 5],
    _rsv8: [Mmio<u8>; 12],
    rms: Mmio<u16>,
    _rsv9: Mmio<u32>,
    _c_plus_cr: Mmio<u16>,
    _rsv10: Mmio<u16>,
    rdsar: [Mmio<u32>; 2],
    mtps: Mmio<u8>,
    _rsv11: [Mmio<u8>; 19],
}

const OWN: u32 = 1 << 31;
const EOR: u32 = 1 << 30;
const FS: u32 = 1 << 29;
const LS: u32 = 1 << 28;

#[repr(C, packed)]
struct Rd {
    ctrl: Mmio<u32>,
    _vlan: Mmio<u32>,
    buffer_low: Mmio<u32>,
    buffer_high: Mmio<u32>,
}

#[repr(C, packed)]
struct Td {
    ctrl: Mmio<u32>,
    _vlan: Mmio<u32>,
    buffer_low: Mmio<u32>,
    buffer_high: Mmio<u32>,
}

pub struct Rtl8168 {
    regs: &'static mut Regs,
    receive_buffer: [Dma<[Mmio<u8>; 0x1FF8]>; 64],
    receive_ring: Dma<[Rd; 64]>,
    receive_i: usize,
    transmit_buffer: [Dma<[Mmio<u8>; 7552]>; 16],
    transmit_ring: Dma<[Td; 16]>,
    transmit_i: usize,
    transmit_buffer_h: [Dma<[Mmio<u8>; 7552]>; 1],
    transmit_ring_h: Dma<[Td; 1]>,
    mac_address: [u8; 6],
}

impl NetworkAdapter for Rtl8168 {
    fn mac_address(&mut self) -> [u8; 6] {
        self.mac_address
    }

    fn available_for_read(&mut self) -> usize {
        self.next_read()
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        if self.receive_i >= self.receive_ring.len() {
            self.receive_i = 0;
        }

        let rd = &mut self.receive_ring[self.receive_i];
        if !rd.ctrl.readf(OWN) {
            let rd_len = rd.ctrl.read() & 0x3FFF;

            let data = &self.receive_buffer[self.receive_i];

            let mut i = 0;
            while i < buf.len() && i < rd_len as usize {
                buf[i] = data[i].read();
                i += 1;
            }

            let eor = rd.ctrl.read() & EOR;
            rd.ctrl.write(OWN | eor | data.len() as u32);

            self.receive_i += 1;

            Ok(Some(i))
        } else {
            Ok(None)
        }
    }

    fn write_packet(&mut self, buf: &[u8]) -> Result<usize> {
        loop {
            if self.transmit_i >= self.transmit_ring.len() {
                self.transmit_i = 0;
            }

            let td = &mut self.transmit_ring[self.transmit_i];
            if !td.ctrl.readf(OWN) {
                let data = &mut self.transmit_buffer[self.transmit_i];

                if buf.len() > data.len() {
                    return Err(Error::new(EMSGSIZE));
                }

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    data[i].write(buf[i]);
                    i += 1;
                }

                let eor = td.ctrl.read() & EOR;
                td.ctrl.write(OWN | eor | FS | LS | i as u32);

                self.regs.tppoll.writef(1 << 6, true); //Notify of normal priority packet

                while self.regs.tppoll.readf(1 << 6) {
                    std::hint::spin_loop();
                }

                self.transmit_i += 1;

                return Ok(i);
            }

            std::hint::spin_loop();
        }
    }
}

impl Rtl8168 {
    pub unsafe fn new(base: usize) -> Result<Self> {
        assert_eq!(mem::size_of::<Regs>(), 256);

        let regs = &mut *(base as *mut Regs);
        assert_eq!(&regs.tnpds as *const _ as usize - base, 0x20);
        assert_eq!(&regs.cmd as *const _ as usize - base, 0x37);
        assert_eq!(&regs.tcr as *const _ as usize - base, 0x40);
        assert_eq!(&regs.rcr as *const _ as usize - base, 0x44);
        assert_eq!(&regs.cmd_9346 as *const _ as usize - base, 0x50);
        assert_eq!(&regs.phys_sts as *const _ as usize - base, 0x6C);
        assert_eq!(&regs.rms as *const _ as usize - base, 0xDA);
        assert_eq!(&regs.rdsar as *const _ as usize - base, 0xE4);
        assert_eq!(&regs.mtps as *const _ as usize - base, 0xEC);

        let mut module = Rtl8168 {
            regs,
            receive_buffer: (0..64)
                .map(|_| Ok(Dma::zeroed()?.assume_init()))
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .unwrap_or_else(|_| unreachable!()),

            receive_ring: Dma::zeroed()?.assume_init(),
            receive_i: 0,
            transmit_buffer: (0..16)
                .map(|_| Ok(Dma::zeroed()?.assume_init()))
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .unwrap_or_else(|_| unreachable!()),
            transmit_ring: Dma::zeroed()?.assume_init(),
            transmit_i: 0,
            transmit_buffer_h: [Dma::zeroed()?.assume_init()],
            transmit_ring_h: Dma::zeroed()?.assume_init(),
            mac_address: [0; 6],
        };

        module.init();

        Ok(module)
    }

    pub unsafe fn irq(&mut self) -> bool {
        // Read and then clear the ISR
        let isr = self.regs.isr.read();
        self.regs.isr.write(isr);
        let imr = self.regs.imr.read();
        (isr & imr) != 0
    }

    pub fn next_read(&self) -> usize {
        let mut receive_i = self.receive_i;
        if receive_i >= self.receive_ring.len() {
            receive_i = 0;
        }

        let rd = &self.receive_ring[receive_i];
        if !rd.ctrl.readf(OWN) {
            (rd.ctrl.read() & 0x3FFF) as usize
        } else {
            0
        }
    }

    pub unsafe fn init(&mut self) -> Result<()> {
        let mac_low = self.regs.mac[0].read();
        let mac_high = self.regs.mac[1].read();
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

        // Reset - this will disable tx and rx, reinitialize FIFOs, and set the system buffer pointer to the initial value
        {
            log::debug!("Reset");
            let timeout = Timeout::from_secs(1);
            self.regs.cmd.writef(1 << 4, true);
            while self.regs.cmd.readf(1 << 4) {
                timeout.run().map_err(|()| Error::new(EIO))?;
            }
        }

        // Set up rx buffers
        log::debug!("Receive buffers");
        for i in 0..self.receive_ring.len() {
            let rd = &mut self.receive_ring[i];
            let data = &mut self.receive_buffer[i];
            rd.buffer_low.write(data.physical() as u32);
            rd.buffer_high.write((data.physical() as u64 >> 32) as u32);
            rd.ctrl.write(OWN | data.len() as u32);
        }
        if let Some(rd) = self.receive_ring.last_mut() {
            rd.ctrl.writef(EOR, true);
        }

        // Set up normal priority tx buffers
        log::debug!("Transmit buffers (normal priority)");
        for i in 0..self.transmit_ring.len() {
            self.transmit_ring[i]
                .buffer_low
                .write(self.transmit_buffer[i].physical() as u32);
            self.transmit_ring[i]
                .buffer_high
                .write((self.transmit_buffer[i].physical() as u64 >> 32) as u32);
        }
        if let Some(td) = self.transmit_ring.last_mut() {
            td.ctrl.writef(EOR, true);
        }

        // Set up high priority tx buffers
        log::debug!("Transmit buffers (high priority)");
        for i in 0..self.transmit_ring_h.len() {
            self.transmit_ring_h[i]
                .buffer_low
                .write(self.transmit_buffer_h[i].physical() as u32);
            self.transmit_ring_h[i]
                .buffer_high
                .write((self.transmit_buffer_h[i].physical() as u64 >> 32) as u32);
        }
        if let Some(td) = self.transmit_ring_h.last_mut() {
            td.ctrl.writef(EOR, true);
        }

        log::debug!("Set config");
        // Unlock config
        self.regs.cmd_9346.write(1 << 7 | 1 << 6);

        // Enable rx (bit 3) and tx (bit 2)
        self.regs.cmd.writef(1 << 3 | 1 << 2, true);

        // Max RX packet size
        self.regs.rms.write(0x1FF8);

        // Max TX packet size
        self.regs.mtps.write(0x3B);

        // Set tx low priority buffer address
        self.regs.tnpds[0].write(self.transmit_ring.physical() as u32);
        self.regs.tnpds[1].write(((self.transmit_ring.physical() as u64) >> 32) as u32);

        // Set tx high priority buffer address
        self.regs.thpds[0].write(self.transmit_ring_h.physical() as u32);
        self.regs.thpds[1].write(((self.transmit_ring_h.physical() as u64) >> 32) as u32);

        // Set rx buffer address
        self.regs.rdsar[0].write(self.receive_ring.physical() as u32);
        self.regs.rdsar[1].write(((self.receive_ring.physical() as u64) >> 32) as u32);

        // Disable timer interrupt
        self.regs.timer_int.write(0);

        //Clear ISR
        let isr = self.regs.isr.read();
        self.regs.isr.write(isr);

        // Interrupt on tx error (bit 3), tx ok (bit 2), rx error(bit 1), and rx ok (bit 0)
        self.regs.imr.write(
            1 << 15 | 1 << 14 | 1 << 7 | 1 << 6 | 1 << 5 | 1 << 4 | 1 << 3 | 1 << 2 | 1 << 1 | 1,
        );

        // Set TX config
        self.regs.tcr.write(0b11 << 24 | 0b111 << 8);

        // Set RX config - Accept broadcast (bit 3), multicast (bit 2), and unicast (bit 1)
        self.regs.rcr.write(0xE70E);

        // Lock config
        self.regs.cmd_9346.write(0);

        log::debug!("Complete!");
        Ok(())
    }
}
