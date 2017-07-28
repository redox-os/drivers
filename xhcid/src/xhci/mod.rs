use std::slice;
use syscall::error::Result;
use syscall::io::Io;

mod capability;
mod command;
mod device;
mod doorbell;
mod event;
mod operational;
mod port;
mod runtime;
mod trb;

use self::capability::CapabilityRegs;
use self::command::CommandRing;
use self::device::DeviceList;
use self::doorbell::Doorbell;
use self::operational::OperationalRegs;
use self::port::Port;
use self::runtime::RuntimeRegs;

pub struct Xhci {
    cap: &'static mut CapabilityRegs,
    op: &'static mut OperationalRegs,
    ports: &'static mut [Port],
    dbs: &'static mut [Doorbell],
    run: &'static mut RuntimeRegs,
    devices: DeviceList,
    cmd: CommandRing,
}

impl Xhci {
    pub fn new(address: usize) -> Result<Xhci> {
        let cap = unsafe { &mut *(address as *mut CapabilityRegs) };
        println!("  - CAP {:X}", address);

        let op_base = address + cap.len.read() as usize;
        let op = unsafe { &mut *(op_base as *mut OperationalRegs) };
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
            max_slots = (hcs_params1 & 0xFF) as u8;
            max_ports = ((hcs_params1 & 0xFF000000) >> 24) as u8;

            println!("  - Max Slots: {}, Max Ports {}", max_slots, max_ports);
        }

        let port_base = op_base + 0x400;
        let ports = unsafe { slice::from_raw_parts_mut(port_base as *mut Port, max_ports as usize) };
        println!("  - PORT {:X}", port_base);

        let db_base = address + cap.db_offset.read() as usize;
        let dbs = unsafe { slice::from_raw_parts_mut(db_base as *mut Doorbell, 256) };
        println!("  - DOORBELL {:X}", db_base);

        let run_base = address + cap.rts_offset.read() as usize;
        let run = unsafe { &mut *(run_base as *mut RuntimeRegs) };
        println!("  - RUNTIME {:X}", run_base);

        let mut xhci = Xhci {
            cap: cap,
            op: op,
            ports: ports,
            dbs: dbs,
            run: run,
            devices: DeviceList::new(max_slots)?,
            cmd: CommandRing::new()?,
        };

        xhci.init(max_slots);

        Ok(xhci)
    }

    pub fn init(&mut self, max_slots: u8) {
        println!("  - Set enabled slots");
        // Set enabled slots
        self.op.config.write(max_slots as u32);
        println!("  - Enabled Slots: {}", self.op.config.read() & 0xFF);

        println!("  - Write DCBAAP");
        // Set device context address array pointer
        self.op.dcbaap.write(self.devices.dcbaap());

        println!("  - Write CRCR");
        // Set command ring control register
        self.op.crcr.write(self.cmd.crcr());

        println!("  - Write ERST");
        // Set event ring segment table registers
        self.run.ints[0].erstsz.write(1);
        self.run.ints[0].erstba.write(self.cmd.events.ste.physical() as u64);
        self.run.ints[0].erdp.write(self.cmd.events.trbs.physical() as u64);

        println!("  - Start");
        // Set run/stop to 1
        self.op.usb_cmd.writef(1, true);

        println!("  - Wait for running");
        // Wait until controller is running
        while self.op.usb_sts.readf(1) {
            println!("  - Waiting for XHCI running");
        }

        println!("  - Ring doorbell");
        // Ring command doorbell
        self.dbs[0].write(0);

        println!("  - XHCI initialized");
    }

    pub fn probe(&mut self) {
        for (i, port) in self.ports.iter().enumerate() {
            let data = port.read();
            let state = port.state();
            let speed = port.speed();
            let flags = port.flags();
            println!("   + XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}", i, data, state, speed, flags);
        }

        println!("  - Running Enable Slot command");

        self.cmd.trbs[0].enable_slot(0, true);

        println!("  - Command");
        println!("  - data: {:X}", self.cmd.trbs[0].data.read());
        println!("  - status: {:X}", self.cmd.trbs[0].status.read());
        println!("  - control: {:X}", self.cmd.trbs[0].control.read());

        self.dbs[0].write(0);

        println!("  - Wait for command completion");
        while self.op.crcr.readf(1 << 3) {
            println!("  - Waiting for command completion");
        }

        println!("  - Response");
        println!("  - data: {:X}", self.cmd.events.trbs[0].data.read());
        println!("  - status: {:X}", self.cmd.events.trbs[0].status.read());
        println!("  - control: {:X}", self.cmd.events.trbs[0].control.read());
    }
}
