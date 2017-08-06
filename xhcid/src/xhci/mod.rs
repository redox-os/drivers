use plain::Plain;
use std::{mem, slice};
use syscall::error::Result;
use syscall::io::{Dma, Io};
use usb;

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
use self::trb::TransferKind;

struct Device<'a> {
    trbs: Dma<[trb::Trb; 256]>,
    trb_i: usize,
    cmd: &'a mut CommandRing,
    db: &'a mut Doorbell,
}

impl<'a> Device<'a> {
    fn get_desc<T>(&mut self, kind: usb::DescriptorKind, index: u8, desc: &mut Dma<T>) {
        let len = mem::size_of::<T>();

        self.trbs[self.trb_i].setup(
            usb::Setup::get_descriptor(kind, index, 0, len as u16),
            TransferKind::In, true
        );

        self.trbs[self.trb_i + 1].data(desc.physical(), len as u16, true, true);

        self.trbs[self.trb_i + 2].status(false, true);

        self.db.write(1);

        let event = self.cmd.next_event();
        while event.data.read() == 0 {
            println!("  - Waiting for event");
        }

        for _i in 0..3 {
            self.trbs[self.trb_i].reserved(false);
            self.trb_i += 1;
        }
        event.reserved(false);
    }

    fn get_string(&mut self, index: u8) -> Result<String> {
        let mut sdesc = Dma::<(u8, u8, [u16; 127])>::zeroed()?;
        self.get_desc(
            usb::DescriptorKind::String,
            index,
            &mut sdesc
        );

        let len = sdesc.0 as usize;
        if len > 2 {
            Ok(String::from_utf16(&sdesc.2[.. (len - 2)/2]).unwrap_or(String::new()))
        } else {
            Ok(String::new())
        }
    }
}

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

            println!("  - Reset");
            op.usb_cmd.writef(1 << 1, true);
            while op.usb_sts.readf(1 << 1) {
                println!("  - Waiting for XHCI reset");
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
        // Set enabled slots
        println!("  - Set enabled slots to {}", max_slots);
        self.op.config.write(max_slots as u32);
        println!("  - Enabled Slots: {}", self.op.config.read() & 0xFF);

        // Set device context address array pointer
        println!("  - Write DCBAAP");
        self.op.dcbaap.write(self.devices.dcbaap());

        // Set command ring control register
        println!("  - Write CRCR");
        self.op.crcr.write(self.cmd.crcr());

        // Set event ring segment table registers
        println!("  - Interrupter 0: {:X}", self.run.ints.as_ptr() as usize);
        println!("  - Write ERSTZ");
        self.run.ints[0].erstsz.write(1);
        println!("  - Write ERDP");
        self.run.ints[0].erdp.write(self.cmd.events.trbs.physical() as u64);
        println!("  - Write ERSTBA: {:X}", self.cmd.events.ste.physical() as u64);
        self.run.ints[0].erstba.write(self.cmd.events.ste.physical() as u64);

        // Set run/stop to 1
        println!("  - Start");
        self.op.usb_cmd.writef(1, true);

        // Wait until controller is running
        println!("  - Wait for running");
        while self.op.usb_sts.readf(1) {
            println!("  - Waiting for XHCI running");
        }

        // Ring command doorbell
        println!("  - Ring doorbell");
        self.dbs[0].write(0);

        println!("  - XHCI initialized");
    }

    pub fn probe(&mut self) -> Result<()> {
        for (i, port) in self.ports.iter().enumerate() {
            let data = port.read();
            let state = port.state();
            let speed = port.speed();
            let flags = port.flags();
            println!("   + XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}", i, data, state, speed, flags);

            if flags.contains(port::PORT_CCS) {
                //TODO: Link TRB when running to the end of the ring buffer

                println!("  - Enable slot");

                let slot;
                {
                    let (cmd, event) = self.cmd.next();

                    cmd.enable_slot(0, true);
                    println!("  - Command: {}", cmd);

                    self.dbs[0].write(0);

                    while event.data.read() == 0 {
                        println!("  - Waiting for event");
                    }
                    println!("  - Response: {}", event);
                    slot = (event.control.read() >> 24) as u8;

                    cmd.reserved(false);
                    event.reserved(false);
                }

                println!("  - Slot {}", slot);

                let mut trbs = Dma::<[trb::Trb; 256]>::zeroed()?;
                let mut trb_i = 0;
                let mut input = Dma::<device::InputContext>::zeroed()?;
                {
                    input.add_context.write(1 << 1 | 1);

                    input.device.slot.a.write(1 << 27);
                    input.device.slot.b.write(((i as u32 + 1) & 0xFF) << 16);
                    println!("{:>08X}", input.device.slot.b.read());

                    input.device.endpoints[0].b.write(4096 << 16 | 4 << 3 | 3 << 1);
                    input.device.endpoints[0].trh.write((trbs.physical() >> 32) as u32);
                    input.device.endpoints[0].trl.write(trbs.physical() as u32 | 1);
                }

                {
                    let (cmd, event) = self.cmd.next();

                    cmd.address_device(slot, input.physical(), true);
                    println!("  - Command: {}", cmd);

                    self.dbs[0].write(0);

                    while event.data.read() == 0 {
                        println!("  - Waiting for event");
                    }
                    println!("  - Response: {}", event);

                    cmd.reserved(false);
                    event.reserved(false);
                }

                let mut dev = Device {
                    trbs: trbs,
                    trb_i: trb_i,
                    cmd: &mut self.cmd,
                    db: &mut self.dbs[slot as usize],
                };

                println!("  - Get descriptor");

                let mut ddesc = Dma::<usb::DeviceDescriptor>::zeroed()?;
                dev.get_desc(
                    usb::DescriptorKind::Device,
                    0,
                    &mut ddesc
                );
                println!("{:?}", *ddesc);

                if ddesc.manufacturer_str > 0 {
                    println!("  Manufacturer: {}", dev.get_string(ddesc.manufacturer_str)?);
                }

                if ddesc.product_str > 0 {
                    println!("  Product: {}", dev.get_string(ddesc.product_str)?);
                }

                if ddesc.serial_str > 0 {
                    println!("  Serial: {}", dev.get_string(ddesc.serial_str)?);
                }

                for config in 0..ddesc.configurations {
                    let mut cdesc = Dma::<(usb::ConfigDescriptor, [u8; 4087])>::zeroed()?;
                    dev.get_desc(
                        usb::DescriptorKind::Configuration,
                        config,
                        &mut cdesc
                    );
                    println!("  {}: {:?}", config, cdesc.0);

                    if cdesc.0.configuration_str > 0 {
                        println!("    Name: {}", dev.get_string(cdesc.0.configuration_str)?);
                    }

                    if cdesc.0.total_length as usize > mem::size_of::<usb::ConfigDescriptor>() {
                        let len = cdesc.0.total_length as usize - mem::size_of::<usb::ConfigDescriptor>();
                        let data = &cdesc.1[..len];

                        let mut i = 0;
                        for interface in 0..cdesc.0.interfaces {
                            let mut idesc = usb::InterfaceDescriptor::default();
                            if i < data.len() && idesc.copy_from_bytes(&data[i..]).is_ok() {
                                i += mem::size_of_val(&idesc);
                                println!("    {}: {:?}", interface, idesc);

                                if idesc.interface_str > 0 {
                                    println!("      Name: {}", dev.get_string(idesc.interface_str)?);
                                }

                                for endpoint in 0..idesc.endpoints {
                                    let mut edesc = usb::EndpointDescriptor::default();
                                    if i < data.len() && edesc.copy_from_bytes(&data[i..]).is_ok() {
                                        i += mem::size_of_val(&edesc);
                                        println!("      {}: {:?}", endpoint, edesc);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
