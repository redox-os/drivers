use std::convert::TryInto;
use std::mem;

use driver_network::NetworkAdapter;
use syscall::error::{Error, Result, EIO, EMSGSIZE};

use common::dma::Dma;
use common::io::{Io, Mmio, ReadOnly};
use common::timeout::Timeout;

const RX_BUFFER_SIZE: usize = 64 * 1024;

const RXSTS_ROK: u16 = 1 << 0;

const TSD_TOK: u32 = 1 << 15;
const TSD_OWN: u32 = 1 << 13;
const TSD_SIZE_MASK: u32 = 0x1FFF;

const CR_RST: u8 = 1 << 4;
const CR_RE: u8 = 1 << 3;
const CR_TE: u8 = 1 << 2;
const CR_BUFE: u8 = 1 << 0;

const IMR_TOK: u16 = 1 << 2;
const IMR_ROK: u16 = 1 << 0;

const RCR_RBLEN_8K: u32 = 0b00 << 11;
const RCR_RBLEN_16K: u32 = 0b01 << 11;
const RCR_RBLEN_32K: u32 = 0b10 << 11;
const RCR_RBLEN_64K: u32 = 0b11 << 11;
const RCR_RBLEN_MASK: u32 = 0b11 << 11;
const RCR_AER: u32 = 1 << 5;
const RCR_AR: u32 = 1 << 4;
const RCR_AB: u32 = 1 << 3;
const RCR_AM: u32 = 1 << 2;
const RCR_APM: u32 = 1 << 1;
const RCR_AAP: u32 = 1 << 0;

#[repr(C, packed)]
struct Regs {
    mac: [Mmio<u32>; 2],
    mar: [Mmio<u32>; 2],
    tsd: [Mmio<u32>; 4],
    tsad: [Mmio<u32>; 4],
    rbstart: Mmio<u32>,
    erbcr: ReadOnly<Mmio<u16>>,
    ersr: ReadOnly<Mmio<u8>>,
    cr: Mmio<u8>,
    capr: Mmio<u16>,
    cbr: ReadOnly<Mmio<u16>>,
    imr: Mmio<u16>,
    isr: Mmio<u16>,
    tcr: Mmio<u32>,
    rcr: Mmio<u32>,
    tctr: Mmio<u32>,
    mpc: Mmio<u32>,
    cr_9346: Mmio<u8>,
    config0: Mmio<u8>,
    config1: Mmio<u8>,
    rsvd_53: ReadOnly<Mmio<u8>>,
    timer_int: Mmio<u32>,
    msr: Mmio<u8>,
    config2: Mmio<u8>,
    config3: Mmio<u8>,
    rsvd_5b: ReadOnly<Mmio<u8>>,
    mulint: Mmio<u16>,
    rerid: ReadOnly<Mmio<u8>>,
    rsvd_5f: ReadOnly<Mmio<u8>>,
    tsts: ReadOnly<Mmio<u16>>,
    _todo: [ReadOnly<Mmio<u8>>; 158],
}

impl Regs {
    unsafe fn from_base(base: usize) -> &'static mut Self {
        assert_eq!(mem::size_of::<Regs>(), 256);

        let regs = &mut *(base as *mut Regs);

        assert_eq!(&regs.mac[0] as *const _ as usize - base, 0x00);
        assert_eq!(&regs.mac[1] as *const _ as usize - base, 0x04);
        assert_eq!(&regs.mar[0] as *const _ as usize - base, 0x08);
        assert_eq!(&regs.mar[1] as *const _ as usize - base, 0x0C);
        assert_eq!(&regs.tsd[0] as *const _ as usize - base, 0x10);
        assert_eq!(&regs.tsd[1] as *const _ as usize - base, 0x14);
        assert_eq!(&regs.tsd[2] as *const _ as usize - base, 0x18);
        assert_eq!(&regs.tsd[3] as *const _ as usize - base, 0x1C);
        assert_eq!(&regs.tsad[0] as *const _ as usize - base, 0x20);
        assert_eq!(&regs.tsad[1] as *const _ as usize - base, 0x24);
        assert_eq!(&regs.tsad[2] as *const _ as usize - base, 0x28);
        assert_eq!(&regs.tsad[3] as *const _ as usize - base, 0x2C);
        assert_eq!(&regs.rbstart as *const _ as usize - base, 0x30);
        assert_eq!(&regs.erbcr as *const _ as usize - base, 0x34);
        assert_eq!(&regs.ersr as *const _ as usize - base, 0x36);
        assert_eq!(&regs.cr as *const _ as usize - base, 0x37);
        assert_eq!(&regs.capr as *const _ as usize - base, 0x38);
        assert_eq!(&regs.cbr as *const _ as usize - base, 0x3A);
        assert_eq!(&regs.imr as *const _ as usize - base, 0x3C);
        assert_eq!(&regs.isr as *const _ as usize - base, 0x3E);
        assert_eq!(&regs.tcr as *const _ as usize - base, 0x40);
        assert_eq!(&regs.rcr as *const _ as usize - base, 0x44);
        assert_eq!(&regs.tctr as *const _ as usize - base, 0x48);
        assert_eq!(&regs.mpc as *const _ as usize - base, 0x4C);
        assert_eq!(&regs.cr_9346 as *const _ as usize - base, 0x50);
        assert_eq!(&regs.config0 as *const _ as usize - base, 0x51);
        assert_eq!(&regs.config1 as *const _ as usize - base, 0x52);
        assert_eq!(&regs.rsvd_53 as *const _ as usize - base, 0x53);
        assert_eq!(&regs.timer_int as *const _ as usize - base, 0x54);
        assert_eq!(&regs.msr as *const _ as usize - base, 0x58);
        assert_eq!(&regs.config2 as *const _ as usize - base, 0x59);
        assert_eq!(&regs.config3 as *const _ as usize - base, 0x5A);
        assert_eq!(&regs.rsvd_5b as *const _ as usize - base, 0x5B);
        assert_eq!(&regs.mulint as *const _ as usize - base, 0x5C);
        assert_eq!(&regs.rerid as *const _ as usize - base, 0x5E);
        assert_eq!(&regs.rsvd_5f as *const _ as usize - base, 0x5F);
        assert_eq!(&regs.tsts as *const _ as usize - base, 0x60);

        regs
    }
}

pub struct Rtl8139 {
    regs: &'static mut Regs,
    receive_buffer: Dma<[Mmio<u8>; RX_BUFFER_SIZE + 16]>,
    receive_i: usize,
    transmit_buffer: [Dma<[Mmio<u8>; 1792]>; 4],
    transmit_i: usize,
    mac_address: [u8; 6],
}

impl NetworkAdapter for Rtl8139 {
    fn mac_address(&mut self) -> [u8; 6] {
        self.mac_address
    }

    fn available_for_read(&mut self) -> usize {
        self.next_read()
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        if !self.regs.cr.readf(CR_BUFE) {
            let rxsts = (self.rx(0) as u16) | (self.rx(1) as u16) << 8;

            let size_with_crc = (self.rx(2) as usize) | (self.rx(3) as usize) << 8;

            let res = if (rxsts & RXSTS_ROK) == RXSTS_ROK {
                let mut i = 0;
                while i < buf.len() && i < size_with_crc.saturating_sub(4) {
                    buf[i] = self.rx(4 + i as u16);
                    i += 1;
                }
                Ok(Some(i))
            } else {
                //TODO: better error types
                log::error!("invalid receive status 0x{:X}", rxsts);
                Err(Error::new(EIO))
            };

            self.receive_i =
                (self.receive_i + 4 + size_with_crc).next_multiple_of(4) % RX_BUFFER_SIZE;
            let capr = self.receive_i.wrapping_sub(16) as u16;
            self.regs.capr.write(capr);

            res
        } else {
            Ok(None)
        }
    }

    fn write_packet(&mut self, buf: &[u8]) -> Result<usize> {
        loop {
            if self.transmit_i >= 4 {
                self.transmit_i = 0;
            }

            if self.regs.tsd[self.transmit_i].readf(TSD_OWN) {
                let data = &mut self.transmit_buffer[self.transmit_i];

                if buf.len() > data.len() {
                    return Err(Error::new(EMSGSIZE));
                }

                let mut i = 0;
                while i < buf.len() && i < data.len() {
                    data[i].write(buf[i]);
                    i += 1;
                }

                self.regs.tsad[self.transmit_i].write(data.physical() as u32);
                assert_eq!(i as u32, i as u32 & TSD_SIZE_MASK);
                self.regs.tsd[self.transmit_i].write(i as u32 & TSD_SIZE_MASK);

                //TODO: wait for TSD_TOK or error

                self.transmit_i += 1;

                return Ok(i);
            }

            std::hint::spin_loop();
        }
    }
}

impl Rtl8139 {
    pub unsafe fn new(base: usize) -> Result<Self> {
        let regs = Regs::from_base(base);

        let mut module = Rtl8139 {
            regs,
            //TODO: limit to 32-bit
            receive_buffer: Dma::zeroed().map(|dma| dma.assume_init())?,
            receive_i: 0,
            //TODO: limit to 32-bit
            transmit_buffer: (0..4)
                .map(|_| Ok(Dma::zeroed()?.assume_init()))
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .unwrap_or_else(|_| unreachable!()),
            transmit_i: 0,
            mac_address: [0; 6],
        };

        module.init()?;

        Ok(module)
    }

    pub unsafe fn irq(&mut self) -> bool {
        // Read and then clear the ISR
        let isr = self.regs.isr.read();
        self.regs.isr.write(isr);
        let imr = self.regs.imr.read();
        (isr & imr) != 0
    }

    fn rx(&self, offset: u16) -> u8 {
        let index = (self.receive_i + offset as usize) % RX_BUFFER_SIZE;
        self.receive_buffer[index].read()
    }

    pub fn next_read(&self) -> usize {
        if !self.regs.cr.readf(CR_BUFE) {
            let rxsts = (self.rx(0) as u16) | (self.rx(1) as u16) << 8;

            let size_with_crc = (self.rx(2) as usize) | (self.rx(3) as usize) << 8;

            if (rxsts & RXSTS_ROK) == RXSTS_ROK {
                size_with_crc.saturating_sub(4)
            } else {
                0
            }
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
            self.regs.cr.writef(CR_RST, true);
            while self.regs.cr.readf(CR_RST) {
                timeout.run().map_err(|()| Error::new(EIO))?;
            }
        }

        // Set up rx buffer
        log::debug!("Receive buffer");
        self.regs
            .rbstart
            .write(self.receive_buffer.physical() as u32);

        log::debug!("Interrupt mask");
        self.regs.imr.write(IMR_TOK | IMR_ROK);

        log::debug!("Receive configuration");
        self.regs
            .rcr
            .write(RCR_RBLEN_64K | RCR_AB | RCR_AM | RCR_APM | RCR_AAP);

        log::debug!("Enable RX and TX");
        self.regs.cr.writef(CR_RE | CR_TE, true);

        log::debug!("Complete!");
        Ok(())
    }
}
