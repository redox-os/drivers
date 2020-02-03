use std::{mem, slice, process, sync::atomic, task};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak, atomic::AtomicBool};

use serde::Deserialize;
use syscall::error::{EBADF, EBADMSG, ENOENT, Error, Result};
use syscall::io::{Dma, Io};

use crate::usb;

mod capability;
mod command;
mod context;
mod doorbell;
mod event;
mod operational;
mod port;
mod runtime;
mod ring;
mod scheme;
mod trb;

use self::capability::CapabilityRegs;
use self::command::CommandRing;
use self::context::{DeviceContextList, InputContext, StreamContextArray};
use self::doorbell::Doorbell;
use self::operational::OperationalRegs;
use self::port::Port;
use self::ring::Ring;
use self::runtime::{RuntimeRegs, Interrupter};
use self::trb::{TransferKind, TrbCompletionCode, TrbType};

struct Device<'a> {
    ring: &'a mut Ring,
    cmd: &'a mut CommandRing,
    db: &'a mut Doorbell,
    int: &'a mut Interrupter,
}

impl<'a> Device<'a> {
    fn get_desc<T>(&mut self, kind: usb::DescriptorKind, index: u8, desc: &mut Dma<T>) {
        let len = mem::size_of::<T>();

        {
            let (cmd, cycle) = self.ring.next();
            cmd.setup(
                usb::Setup::get_descriptor(kind, index, 0, len as u16),
                TransferKind::In, cycle
            );
        }

        {
            let (cmd, cycle) = self.ring.next();
            cmd.data(desc.physical(), len as u16, true, cycle);
        }

        {
            let (cmd, cycle) = self.ring.next();
            cmd.status(false, cycle);
        }

        self.db.write(1);

        {
            let event = self.cmd.next_event();
            while event.data.read() == 0 {
                println!("  - Waiting for event");
            }
        }

        self.int.erdp.write(self.cmd.erdp());
    }

    fn get_device(&mut self) -> Result<usb::DeviceDescriptor> {
        let mut desc = Dma::<usb::DeviceDescriptor>::zeroed()?;
        self.get_desc(
            usb::DescriptorKind::Device,
            0,
            &mut desc
        );
        Ok(*desc)
    }

    fn get_config(&mut self, config: u8) -> Result<(usb::ConfigDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::ConfigDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc(
            usb::DescriptorKind::Configuration,
            config,
            &mut desc
        );
        Ok(*desc)
    }

    fn get_bos(&mut self) -> Result<(usb::BosDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::BosDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc(
            usb::DescriptorKind::BinaryObjectStorage,
            0,
            &mut desc,
        );
        Ok(*desc)
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
    dev_ctx: DeviceContextList,
    cmd: CommandRing,

    handles: BTreeMap<usize, scheme::Handle>,
    next_handle: usize,
    port_states: BTreeMap<usize, PortState>,

    // TODO: Is this the correct implementation? I mean, there will be a really limited number of
    // IRQs, if not just one, and since we probably wont use a thread pool scheduler like those of
    // async-std or tokio, one could possibly assume that the futures themselves won't have to push
    // all the wakers.
    // TODO: This should probably be a BTreeMap (or just a VecMap) of states for each IRQ number,
    // if more than one are used. I'm not sure if the XHCI interrupters actually use different
    // IRQs, but it would make sense in case the hub has both isochronous (which trigger interrupts
    // reapeatedly with some time in between), bulk, control, etc. I might be wrong though...
    irq_state: Arc<IrqState>,

    drivers: BTreeMap<usize, process::Child>,
    scheme_name: String,
}

struct PortState {
    slot: u8,
    input_context: Dma<InputContext>,
    dev_desc: scheme::DevDesc,
    endpoint_states: BTreeMap<u8, EndpointState>,
}

pub(crate) enum RingOrStreams {
    Ring(Ring),
    Streams(StreamContextArray),
}

pub(crate) enum EndpointState {
    Ready(RingOrStreams),
    Init,
}
impl EndpointState {
    fn ring(&mut self) -> Option<&mut Ring> {
        match self {
            Self::Ready(RingOrStreams::Ring(ring)) => Some(ring),
            _ => None,
        }
    }
}

impl Xhci {
    pub fn new(scheme_name: String, address: usize) -> Result<Xhci> {
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
            dev_ctx: DeviceContextList::new(max_slots)?,
            cmd: CommandRing::new()?,
            handles: BTreeMap::new(),
            next_handle: 0,
            port_states: BTreeMap::new(),

            irq_state: Arc::new(IrqState {
                triggered: AtomicBool::new(false),
                wakers: Mutex::new(Vec::new()),
            }),
            drivers: BTreeMap::new(),
            scheme_name,
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
        let dcbaap = self.dev_ctx.dcbaap();
        println!("  - Write DCBAAP: {:X}", dcbaap);
        self.op.dcbaap.write(dcbaap as u64);

        // Set command ring control register
        let crcr = self.cmd.crcr();
        println!("  - Write CRCR: {:X}", crcr);
        self.op.crcr.write(crcr as u64);

        // Set event ring segment table registers
        println!("  - Interrupter 0: {:X}", self.run.ints.as_ptr() as usize);
        {
            let erstz = 1;
            println!("  - Write ERSTZ: {}", erstz);
            self.run.ints[0].erstsz.write(erstz);

            let erdp = self.cmd.erdp();
            println!("  - Write ERDP: {:X}", erdp);
            self.run.ints[0].erdp.write(erdp as u64);

            let erstba = self.cmd.erstba();
            println!("  - Write ERSTBA: {:X}", erstba);
            self.run.ints[0].erstba.write(erstba as u64);

            println!("  - Enable interrupts");
            self.run.ints[0].iman.writef(1 << 1, true);
        }

        // Set run/stop to 1
        println!("  - Start");
        self.op.usb_cmd.writef(1 | 1 << 2, true);

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

    pub fn enable_port_slot(cmd: &mut CommandRing, dbs: &mut [Doorbell]) -> u8 {
        let (cmd, cycle, event) = cmd.next();

        cmd.enable_slot(0, cycle);

        dbs[0].write(0);

        while event.data.read() == 0 {
            println!("    - Waiting for event");
        }
        let slot = (event.control.read() >> 24) as u8;

        cmd.reserved(false);
        event.reserved(false);

        slot
    }

    pub fn slot_state(&self, slot: usize) -> u8 {
        self.dev_ctx.contexts[slot].slot.state()
    }

    pub fn probe(&mut self) -> Result<()> {
        for i in 0..self.ports.len() {
            let (data, state, speed, flags) = {
                let port = &self.ports[i];
                (
                    port.read(),
                    port.state(),
                    port.speed(),
                    port.flags(),
                )
            };
            println!("   + XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}", i, data, state, speed, flags);

            if flags.contains(port::PortFlags::PORT_CCS) {
                //TODO: Link TRB when running to the end of the ring buffer

                println!("    - Enable slot");

                self.run.ints[0].erdp.write(self.cmd.erdp());

                let slot = Self::enable_port_slot(&mut self.cmd, &mut self.dbs);

                println!("    - Slot {}", slot);

                // transfer ring?
                let mut ring = Ring::new(true)?;

                let mut input = Dma::<InputContext>::zeroed()?;
                {
                    input.add_context.write(1 << 1 | 1); // Enable the slot (zeroth bit) and the control endpoint (first bit).

                    input.device.slot.a.write((1 << 27) | (speed << 20)); // FIXME: The speed field, bits 23:20, is deprecated.
                    input.device.slot.b.write(((i as u32 + 1) & 0xFF) << 16);

                    // control endpoint?
                    input.device.endpoints[0].b.write(4096 << 16 | 4 << 3 | 3 << 1); // packet size | control endpoint | allowed error count
                    let tr = ring.register();
                    input.device.endpoints[0].trh.write((tr >> 32) as u32);
                    input.device.endpoints[0].trl.write(tr as u32);
                }

                {
                    let (cmd, cycle, event) = self.cmd.next();

                    cmd.address_device(slot, input.physical(), cycle);

                    self.dbs[0].write(0);

                    while event.data.read() == 0 {
                        println!("    - Waiting for event");
                    }

                    if event.completion_code() != TrbCompletionCode::Success as u8 || event.trb_type() != TrbType::CommandCompletion as u8 {
                        panic!("ADDRESS DEVICE FAILED");
                    }

                    cmd.reserved(false);
                    event.reserved(false);
                }

                let dev_desc = Self::get_dev_desc_raw(&mut self.ports, &mut self.run, &mut self.cmd, &mut self.dbs, i, slot, &mut ring)?;
                let mut port_state = PortState {
                    slot,
                    input_context: input,
                    dev_desc,
                    endpoint_states: std::iter::once((0, EndpointState::Ready(
                        RingOrStreams::Ring(ring),
                    ))).collect::<BTreeMap<_, _>>(),
                };

                match self.spawn_drivers(i, &mut port_state) {
                    Ok(()) => (),
                    Err(err) => println!("Failed to spawn driver for port {}: `{}`", i, err),
                }

                self.port_states.insert(i, port_state);
            }
        }

        Ok(())
    }

    pub fn trigger_irq(&mut self) -> bool {
        // Read the Interrupter Pending bit.
        println!("preinterrupt");
        if self.run.ints[0].iman.readf(1) {
            println!("XHCI Interrupt");

            // If set, set it back to zero, so that new interrupts can be triggered.
            // FIXME: MSI and MSI-X systems
            self.run.ints[0].iman.writef(1, true);

            // Wake all futures awaiting the IRQ.
            for waker in self.irq_state.wakers.lock().unwrap().drain(..) {
                waker.wake();
            }

            true
        } else {
            false
        }
    }
    pub(crate) fn irq(&self) -> IrqFuture {
        IrqFuture {
            state: IrqFutureState::Pending(Arc::downgrade(&self.irq_state))
        }
    }
    fn spawn_drivers(&mut self, port: usize, ps: &mut PortState) -> Result<()> {
        // TODO: There should probably be a way to select alternate interfaces, and not just the
        // first one.
        // TODO: Now that there are some good error crates, I don't think errno.h error codes are
        // suitable here.

        let ifdesc = &ps.dev_desc.config_descs.first().ok_or(Error::new(EBADF))?.interface_descs.first().ok_or(Error::new(EBADF))?;
        let drivers_usercfg: &DriversConfig = &DRIVERS_CONFIG;

        if let Some(driver) = drivers_usercfg.drivers.iter().find(|driver| driver.class == ifdesc.class && driver.subclass == ifdesc.sub_class) {
            println!("Loading driver \"{}\"", driver.name);
            let (command, args) = driver.command.split_first().ok_or(Error::new(EBADMSG))?;

            let if_proto = ifdesc.protocol;

            let process = process::Command::new(command).args(args.into_iter().map(|arg| arg.replace("$SCHEME", &self.scheme_name).replace("$PORT", &format!("{}", port)).replace("$IF_PROTO", &format!("{}", if_proto))).collect::<Vec<_>>()).stdin(process::Stdio::null()).spawn().or(Err(Error::new(ENOENT)))?;
            self.drivers.insert(port, process);
        } else {
            return Err(Error::new(ENOENT));
        }

        Ok(())
    }
}
#[derive(Deserialize)]
struct DriverConfig {
    name: String,
    class: u8,
    subclass: u8,
    command: Vec<String>,
}
#[derive(Deserialize)]
struct DriversConfig {
    drivers: Vec<DriverConfig>,
}

use lazy_static::lazy_static;

lazy_static! {
    static ref DRIVERS_CONFIG: DriversConfig = {
        // TODO: Load this at runtime.
        const TOML: &'static [u8] = include_bytes!("../../drivers.toml");

        toml::from_slice::<DriversConfig>(TOML).expect("Failed to parse internally embedded config file")
    };
}

pub(crate) struct IrqFuture { state: IrqFutureState }

struct IrqState {
    triggered: AtomicBool,
    // TODO: Perhaps a channel?
    wakers: Mutex<Vec<task::Waker>>,
}

enum IrqFutureState {
    Pending(Weak<IrqState>),
    Finished,
}

impl std::future::Future for IrqFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        let this = self.get_mut();

        match &mut this.state {
            // TODO: Ordering?
            IrqFutureState::Pending(state_weak) => {
                let state = state_weak.upgrade().expect("IRQ futures keep getting polled even after the driver has been deinitialized");

                if state.triggered.load(atomic::Ordering::SeqCst) {
                    this.state = IrqFutureState::Finished;
                    task::Poll::Ready(())
                } else {
                    state.wakers.lock().unwrap().push(context.waker().clone());
                    task::Poll::Pending
                }
            }
            IrqFutureState::Finished => panic!("polling finished future"),
        }
    }
}
