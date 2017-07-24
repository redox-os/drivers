use std::slice;
use syscall::error::Result;
use syscall::io::{Dma, Mmio, Io};

mod event;
mod trb;

use self::event::*;
use self::trb::*;

#[repr(packed)]
pub struct XhciCap {
    len: Mmio<u8>,
    _rsvd: Mmio<u8>,
    hci_ver: Mmio<u16>,
    hcs_params1: Mmio<u32>,
    hcs_params2: Mmio<u32>,
    hcs_params3: Mmio<u32>,
    hcc_params1: Mmio<u32>,
    db_offset: Mmio<u32>,
    rts_offset: Mmio<u32>,
    hcc_params2: Mmio<u32>
}

#[repr(packed)]
pub struct XhciOp {
    usb_cmd: Mmio<u32>,
    usb_sts: Mmio<u32>,
    page_size: Mmio<u32>,
    _rsvd: [Mmio<u32>; 2],
    dn_ctrl: Mmio<u32>,
    crcr: Mmio<u64>,
    _rsvd2: [Mmio<u32>; 4],
    dcbaap: Mmio<u64>,
    config: Mmio<u32>,
}

pub struct XhciInterrupter {
    iman: Mmio<u32>,
    imod: Mmio<u32>,
    erstsz: Mmio<u32>,
    _rsvd: Mmio<u32>,
    erstba: Mmio<u64>,
    erdp: Mmio<u64>,
}

pub struct XhciRun {
    mfindex: Mmio<u32>,
    _rsvd: [Mmio<u32>; 7],
    ints: [XhciInterrupter; 1024],
}

bitflags! {
    flags XhciPortFlags: u32 {
        const PORT_CCS = 1 << 0,
        const PORT_PED = 1 << 1,
        const PORT_OCA = 1 << 3,
        const PORT_PR =  1 << 4,
        const PORT_PP =  1 << 9,
        const PORT_PIC_AMB = 1 << 14,
        const PORT_PIC_GRN = 1 << 15,
        const PORT_LWS = 1 << 16,
        const PORT_CSC = 1 << 17,
        const PORT_PEC = 1 << 18,
        const PORT_WRC = 1 << 19,
        const PORT_OCC = 1 << 20,
        const PORT_PRC = 1 << 21,
        const PORT_PLC = 1 << 22,
        const PORT_CEC = 1 << 23,
        const PORT_CAS = 1 << 24,
        const PORT_WCE = 1 << 25,
        const PORT_WDE = 1 << 26,
        const PORT_WOE = 1 << 27,
        const PORT_DR =  1 << 30,
        const PORT_WPR = 1 << 31,
    }
}

#[repr(packed)]
pub struct XhciPort {
    portsc : Mmio<u32>,
    portpmsc : Mmio<u32>,
    portli : Mmio<u32>,
    porthlpmc : Mmio<u32>,
}

impl XhciPort {
    fn read(&self) -> u32 {
        self.portsc.read()
    }

    fn state(&self) -> u32 {
        (self.read() & (0b1111 << 5)) >> 5
    }

    fn speed(&self) -> u32 {
        (self.read() & (0b1111 << 10)) >> 10
    }

    fn flags(&self) -> XhciPortFlags {
        XhciPortFlags::from_bits_truncate(self.read())
    }
}

pub struct XhciDoorbell(Mmio<u32>);

impl XhciDoorbell {
    fn read(&self) -> u32 {
        self.0.read()
    }

    fn write(&mut self, data: u32) {
        self.0.write(data);
    }
}

#[repr(packed)]
pub struct XhciSlotContext {
    inner: [u8; 32]
}

#[repr(packed)]
pub struct XhciEndpointContext {
    inner: [u8; 32]
}

#[repr(packed)]
pub struct XhciDeviceContext {
    slot: XhciSlotContext,
    endpoints: [XhciEndpointContext; 15]
}

pub struct Xhci {
    cap: &'static mut XhciCap,
    op: &'static mut XhciOp,
    ports: &'static mut [XhciPort],
    dbs: &'static mut [XhciDoorbell],
    run: &'static mut XhciRun,
    dev_baa: Dma<[u64; 256]>,
    dev_ctxs: Vec<Dma<XhciDeviceContext>>,
    cmds: Dma<[Trb; 256]>,
    events: [EventRing; 1],
}

impl Xhci {
    pub fn new(address: usize) -> Result<Xhci> {
        let cap = unsafe { &mut *(address as *mut XhciCap) };
        println!("  - CAP {:X}", address);

        let op_base = address + cap.len.read() as usize;
        let op = unsafe { &mut *(op_base as *mut XhciOp) };
        println!("  - OP {:X}", op_base);

        let max_slots;
        let max_ports;

        {
            println!("  - Wait for ready");
            // Wait until controller is ready
            while op.usb_sts.readf(1 << 11) {
                println!("  - Waiting for XHCI ready");
            }

            println!("  - Stop");
            // Set run/stop to 0
            op.usb_cmd.writef(1, false);

            println!("  - Wait for not running");
            // Wait until controller not running
            while ! op.usb_sts.readf(1) {
                println!("  - Waiting for XHCI stopped");
            }

            println!("  - Read max slots");
            // Read maximum slots and ports
            let hcs_params1 = cap.hcs_params1.read();
            max_slots = hcs_params1 & 0xFF;
            max_ports = (hcs_params1 & 0xFF000000) >> 24;

            println!("  - Max Slots: {}, Max Ports {}", max_slots, max_ports);

            println!("  - Set enabled slots");
            // Set enabled slots
            op.config.write(max_slots);
            println!("  - Enabled Slots: {}", op.config.read() & 0xFF);
        }

        let port_base = op_base + 0x400;
        let ports = unsafe { slice::from_raw_parts_mut(port_base as *mut XhciPort, max_ports as usize) };
        println!("  - PORT {:X}", port_base);

        let db_base = address + cap.db_offset.read() as usize;
        let dbs = unsafe { slice::from_raw_parts_mut(db_base as *mut XhciDoorbell, 256) };
        println!("  - DOORBELL {:X}", db_base);

        let run_base = address + cap.rts_offset.read() as usize;
        let run = unsafe { &mut *(run_base as *mut XhciRun) };
        println!("  - RUNTIME {:X}", run_base);

        let mut xhci = Xhci {
            cap: cap,
            op: op,
            ports: ports,
            dbs: dbs,
            run: run,
            dev_baa: Dma::zeroed()?,
            dev_ctxs: Vec::new(),
            cmds: Dma::zeroed()?,
            events: [
                EventRing::new()?
            ],
        };

        {
            // Create device context buffers for each slot
            for i in 0..max_slots as usize {
                println!("  - Setup dev ctx {}", i);
                let dev_ctx: Dma<XhciDeviceContext> = Dma::zeroed()?;
                xhci.dev_baa[i] = dev_ctx.physical() as u64;
                xhci.dev_ctxs.push(dev_ctx);
            }

            println!("  - Write DCBAAP");
            // Set device context address array pointer
            xhci.op.dcbaap.write(xhci.dev_baa.physical() as u64);

            println!("  - Write CRCR");
            // Set command ring control register
            xhci.op.crcr.write(xhci.cmds.physical() as u64 | 1);

            println!("  - Write ERST");
            // Set event ring segment table registers
            xhci.run.ints[0].erstsz.write(1);
            xhci.run.ints[0].erstba.write(xhci.events[0].ste.physical() as u64);
            xhci.run.ints[0].erdp.write(xhci.events[0].trbs.physical() as u64);

            println!("  - Start");
            // Set run/stop to 1
            xhci.op.usb_cmd.writef(1, true);

            println!("  - Wait for running");
            // Wait until controller is running
            while xhci.op.usb_sts.readf(1) {
                println!("  - Waiting for XHCI running");
            }

            println!("  - Ring doorbell");
            // Ring command doorbell
            xhci.dbs[0].write(0);

            println!("  - XHCI initialized");
        }

        Ok(xhci)
    }

    pub fn init(&mut self) {
        for (i, port) in self.ports.iter().enumerate() {
            let data = port.read();
            let state = port.state();
            let speed = port.speed();
            let flags = port.flags();
            println!("   + XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}", i, data, state, speed, flags);
        }

        println!("  - Running Enable Slot command");

        self.cmds[0].enable_slot(0, true);

        println!("  - Command");
        println!("  - data: {:X}", self.cmds[0].data.read());
        println!("  - status: {:X}", self.cmds[0].status.read());
        println!("  - control: {:X}", self.cmds[0].control.read());

        self.dbs[0].write(0);

        println!("  - Wait for command completion");
        while self.op.crcr.readf(1 << 3) {
            println!("  - Waiting for command completion");
        }

        println!("  - Response");
        println!("  - data: {:X}", self.events[0].trbs[0].data.read());
        println!("  - status: {:X}", self.events[0].trbs[0].status.read());
        println!("  - control: {:X}", self.events[0].trbs[0].control.read());
    }
}
