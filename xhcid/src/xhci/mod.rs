use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{atomic::AtomicBool, Arc, Mutex, Weak};
use std::{mem, process, slice, sync::atomic, task};

use serde::Deserialize;
use syscall::error::{Error, Result, EBADF, EBADMSG, ENOENT};
use syscall::io::{Dma, Io};

use crate::usb;

mod capability;
mod command;
mod context;
mod doorbell;
mod event;
mod extended;
mod operational;
mod port;
mod ring;
mod runtime;
mod scheme;
mod trb;

use self::capability::CapabilityRegs;
use self::command::CommandRing;
use self::context::{DeviceContextList, InputContext, StreamContextArray};
use self::doorbell::Doorbell;
use self::extended::{CapabilityId, ExtendedCapabilitiesIter, ProtocolSpeed, SupportedProtoCap};
use self::operational::OperationalRegs;
use self::port::Port;
use self::ring::Ring;
use self::runtime::{Interrupter, RuntimeRegs};
use self::trb::{TransferKind, TrbCompletionCode, TrbType};

use self::scheme::EndpIfState;

use crate::driver_interface::*;

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
                TransferKind::In,
                cycle,
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
        self.get_desc(usb::DescriptorKind::Device, 0, &mut desc);
        Ok(*desc)
    }

    fn get_config(&mut self, config: u8) -> Result<(usb::ConfigDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::ConfigDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc(usb::DescriptorKind::Configuration, config, &mut desc);
        Ok(*desc)
    }

    fn get_bos(&mut self) -> Result<(usb::BosDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::BosDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc(usb::DescriptorKind::BinaryObjectStorage, 0, &mut desc);
        Ok(*desc)
    }

    fn get_string(&mut self, index: u8) -> Result<String> {
        let mut sdesc = Dma::<(u8, u8, [u16; 127])>::zeroed()?;
        self.get_desc(usb::DescriptorKind::String, index, &mut sdesc);

        let len = sdesc.0 as usize;
        if len > 2 {
            Ok(String::from_utf16(&sdesc.2[..(len - 2) / 2]).unwrap_or(String::new()))
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

    base: *const u8,

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
    dev_desc: DevDesc,
    endpoint_states: BTreeMap<u8, EndpointState>,
}

pub(crate) enum RingOrStreams {
    Ring(Ring),
    Streams(StreamContextArray),
}

pub(crate) struct EndpointState {
    pub transfer: RingOrStreams,
    pub driver_if_state: EndpIfState,
}
impl EndpointState {
    fn ring(&mut self) -> Option<&mut Ring> {
        match self.transfer {
            RingOrStreams::Ring(ref mut ring) => Some(ring),
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

        let (max_slots, max_ports) = {
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
            while !op.usb_sts.readf(1) {
                println!("  - Waiting for XHCI stopped");
            }

            println!("  - Reset");
            op.usb_cmd.writef(1 << 1, true);
            while op.usb_sts.readf(1 << 1) {
                println!("  - Waiting for XHCI reset");
            }

            println!("  - Read max slots");

            let max_slots = cap.max_slots();
            let max_ports = cap.max_ports();

            println!("  - Max Slots: {}, Max Ports {}", max_slots, max_ports);
            (max_slots, max_ports)
        };

        let port_base = op_base + 0x400;
        let ports =
            unsafe { slice::from_raw_parts_mut(port_base as *mut Port, max_ports as usize) };
        println!("  - PORT {:X}", port_base);

        let db_base = address + cap.db_offset.read() as usize;
        let dbs = unsafe { slice::from_raw_parts_mut(db_base as *mut Doorbell, 256) };
        println!("  - DOORBELL {:X}", db_base);

        let run_base = address + cap.rts_offset.read() as usize;
        let run = unsafe { &mut *(run_base as *mut RuntimeRegs) };
        println!("  - RUNTIME {:X}", run_base);

        let mut xhci = Xhci {
            base: address as *const u8,
            cap,
            op,
            ports,
            dbs,
            run,
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

    pub fn enable_port_slot(&mut self, slot_ty: u8) -> Result<u8> {
        assert_eq!(slot_ty & 0x1F, slot_ty);

        let cloned_event_trb =
            self.execute_command("ENABLE_SLOT", |cmd, cycle| cmd.enable_slot(0, cycle))?;
        Ok(cloned_event_trb.event_slot())
    }
    pub fn disable_port_slot(&mut self, slot: u8) -> Result<()> {
        self.execute_command("DISABLE_SLOT", |cmd, cycle| cmd.enable_slot(0, cycle))?;
        Ok(())
    }

    pub fn slot_state(&self, slot: usize) -> u8 {
        self.dev_ctx.contexts[slot].slot.state()
    }

    pub fn probe(&mut self) -> Result<()> {
        for i in 0..self.ports.len() {
            let (data, state, speed, flags) = {
                let port = &self.ports[i];
                (port.read(), port.state(), port.speed(), port.flags())
            };
            println!(
                "   + XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}",
                i, data, state, speed, flags
            );

            if flags.contains(port::PortFlags::PORT_CCS) {
                //TODO: Link TRB when running to the end of the ring buffer

                println!("    - Enable slot");

                let slot_ty = self
                    .supported_protocol(i as u8)
                    .expect("Failed to find supported protocol information for port")
                    .proto_slot_ty();
                let slot = self.enable_port_slot(slot_ty)?;

                println!("    - Slot {}", slot);

                let mut input = Dma::<InputContext>::zeroed()?;
                let mut ring = self.address_device(&mut input, i, slot_ty, slot, speed)?;

                let dev_desc = Self::get_dev_desc_raw(
                    &mut self.ports,
                    &mut self.run,
                    &mut self.cmd,
                    &mut self.dbs,
                    i,
                    slot,
                    &mut ring,
                )?;

                self.update_default_control_pipe(&mut input, slot, &dev_desc)?;

                let mut port_state = PortState {
                    slot,
                    input_context: input,
                    dev_desc,
                    endpoint_states: std::iter::once((
                        0,
                        EndpointState {
                            transfer: RingOrStreams::Ring(ring),
                            driver_if_state: EndpIfState::Init,
                        },
                    ))
                    .collect::<BTreeMap<_, _>>(),
                };

                if self.cap.cic() {
                    self.op.set_cie(true);
                }

                /*match self.spawn_drivers(i, &mut port_state) {
                    Ok(()) => (),
                    Err(err) => println!("Failed to spawn driver for port {}: `{}`", i, err),
                }*/

                self.port_states.insert(i, port_state);
            }
        }

        Ok(())
    }

    pub fn update_default_control_pipe(
        &mut self,
        input_context: &mut Dma<InputContext>,
        slot_id: u8,
        dev_desc: &DevDesc,
    ) -> Result<()> {
        input_context.add_context.write(1 << 1);
        input_context.drop_context.write(0);

        let new_max_packet_size = if dev_desc.major_version() == 2 {
            u32::from(dev_desc.packet_size)
        } else {
            1u32 << dev_desc.packet_size
        };
        let endp_ctx = &mut input_context.device.endpoints[0];
        let mut b = endp_ctx.b.read();
        b &= 0x0000_FFFF;
        b |= (new_max_packet_size) << 16;
        endp_ctx.b.write(b);

        self.execute_command("EVALUATE_CONTEXT", |trb, cycle| {
            trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
        })?;
        Ok(())
    }

    pub fn address_device(
        &mut self,
        input_context: &mut Dma<InputContext>,
        i: usize,
        slot_ty: u8,
        slot: u8,
        speed: u8,
    ) -> Result<Ring> {
        let mut ring = Ring::new(16, true)?;

        {
            input_context.add_context.write(1 << 1 | 1); // Enable the slot (zeroth bit) and the control endpoint (first bit).

            let slot_ctx = &mut input_context.device.slot;

            let route_string = 0u32; // TODO
            let context_entries = 1u8;
            let mtt = false;
            let hub = false;

            assert_eq!(route_string & 0x000F_FFFF, route_string);
            slot_ctx.a.write(
                route_string
                    | (u32::from(mtt) << 25)
                    | (u32::from(hub) << 26)
                    | (u32::from(context_entries) << 27),
            );

            let max_exit_latency = 0u16;
            let root_hub_port_num = (i + 1) as u8;
            let number_of_ports = 0u8;
            slot_ctx.b.write(
                u32::from(max_exit_latency)
                    | (u32::from(root_hub_port_num) << 16)
                    | (u32::from(number_of_ports) << 24),
            );

            // TODO
            let parent_hud_slot_id = 0u8;
            let parent_port_num = 0u8;
            let ttt = 0u8;
            let interrupter = 0u8;

            assert_eq!(ttt & 0b11, ttt);
            slot_ctx.c.write(
                u32::from(parent_hud_slot_id)
                    | (u32::from(parent_port_num) << 8)
                    | (u32::from(ttt) << 16)
                    | (u32::from(interrupter) << 22),
            );

            let endp_ctx = &mut input_context.device.endpoints[0];

            let speed_id = self
                .lookup_psiv(root_hub_port_num, speed)
                .expect("Failed to retrieve speed ID");

            let max_error_count = 3u8; // recommended value according to the XHCI spec
            let ep_ty = 4u8; // control endpoint, bidirectional
            let max_packet_size: u32 = if speed_id.is_lowspeed() {
                8 // only valid value
            } else if speed_id.is_fullspeed() {
                64 // valid values are 8, 16, 32, 64
            } else if speed_id.is_highspeed() {
                64 // only valid value
            } else if speed_id.is_superspeed_gen_x() {
                512 // only valid value
            } else {
                unreachable!()
            };
            let host_initiate_disable = false; // only applies to streams
            let max_burst_size = 0u8; // TODO

            assert_eq!(max_error_count & 0b11, max_error_count);
            endp_ctx.b.write(
                (u32::from(max_error_count) << 1)
                    | (u32::from(ep_ty) << 3)
                    | (u32::from(host_initiate_disable) << 7)
                    | (u32::from(max_burst_size) << 8)
                    | (u32::from(max_packet_size) << 16),
            );

            let tr = ring.register();
            endp_ctx.trh.write((tr >> 32) as u32);
            endp_ctx.trl.write(tr as u32);
        }

        let input_context_physical = input_context.physical();

        self.execute_command("ADDRESS_DEVICE", |trb, cycle| {
            trb.address_device(slot, input_context_physical, false, cycle)
        })
        .expect("ADDRESS_DEVICE failed");
        Ok(ring)
    }


    pub fn trigger_irq(&mut self) -> bool {
        // Read the Interrupter Pending bit.
        if self.run.ints[0].iman.readf(1) {
            //println!("XHCI Interrupt");

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
            state: IrqFutureState::Pending(Arc::downgrade(&self.irq_state)),
        }
    }
    fn spawn_drivers(&mut self, port: usize, ps: &mut PortState) -> Result<()> {
        // TODO: There should probably be a way to select alternate interfaces, and not just the
        // first one.
        // TODO: Now that there are some good error crates, I don't think errno.h error codes are
        // suitable here.

        let ifdesc = &ps
            .dev_desc
            .config_descs
            .first()
            .ok_or(Error::new(EBADF))?
            .interface_descs
            .first()
            .ok_or(Error::new(EBADF))?;
        let drivers_usercfg: &DriversConfig = &DRIVERS_CONFIG;

        if let Some(driver) = drivers_usercfg.drivers.iter().find(|driver| {
            driver.class == ifdesc.class
                && driver
                    .subclass()
                    .map(|subclass| subclass == ifdesc.sub_class)
                    .unwrap_or(true)
        }) {
            println!("Loading driver \"{}\"", driver.name);
            let (command, args) = driver.command.split_first().ok_or(Error::new(EBADMSG))?;

            let if_proto = ifdesc.protocol;

            let process = process::Command::new(command)
                .args(
                    args.into_iter()
                        .map(|arg| {
                            arg.replace("$SCHEME", &self.scheme_name)
                                .replace("$PORT", &format!("{}", port))
                                .replace("$IF_PROTO", &format!("{}", if_proto))
                        })
                        .collect::<Vec<_>>(),
                )
                .stdin(process::Stdio::null())
                .spawn()
                .or(Err(Error::new(ENOENT)))?;
            self.drivers.insert(port, process);
        } else {
            return Err(Error::new(ENOENT));
        }

        Ok(())
    }
    pub fn capabilities_iter(&self) -> ExtendedCapabilitiesIter {
        unsafe {
            ExtendedCapabilitiesIter::new(
                (self.base as *mut u8).offset((self.cap.ext_caps_ptr_in_dwords() << 2) as isize),
            )
        }
    }
    pub fn supported_protocols_iter(&self) -> impl Iterator<Item = &'static SupportedProtoCap> {
        self.capabilities_iter()
            .filter_map(|(pointer, cap_num)| unsafe {
                if cap_num == CapabilityId::SupportedProtocol as u8 {
                    Some(&*pointer.cast::<SupportedProtoCap>().as_ptr())
                } else {
                    None
                }
            })
    }
    pub fn supported_protocol(&self, port: u8) -> Option<&'static SupportedProtoCap> {
        self.supported_protocols_iter()
            .find(|supp_proto| supp_proto.compat_port_range().contains(&port))
    }
    pub fn supported_protocol_speeds(
        &self,
        port: u8,
    ) -> Option<impl Iterator<Item = &'static ProtocolSpeed>> {
        use extended::*;
        const DEFAULT_SUPP_PROTO_SPEEDS: [ProtocolSpeed; 7] = [
            // Full-speed
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (false as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Mbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 12 << PROTO_SPEED_PSIM_SHIFT
                    | 1 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // Low-speed
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (false as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Kbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 1500 << PROTO_SPEED_PSIM_SHIFT
                    | 2 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // High-speed
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (false as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Mbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 480 << PROTO_SPEED_PSIM_SHIFT
                    | 3 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // SuperSpeed Gen1 x1
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (true as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Gbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 5 << PROTO_SPEED_PSIM_SHIFT
                    | (Lp::SuperSpeed as u32) << PROTO_SPEED_LP_SHIFT
                    | 4 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // SuperSpeedPlus Gen2 x1
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (true as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Gbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 10 << PROTO_SPEED_PSIM_SHIFT
                    | (Lp::SuperSpeedPlus as u32) << PROTO_SPEED_LP_SHIFT
                    | 5 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // SuperSpeedPlus Gen1 x2
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (true as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Gbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 10 << PROTO_SPEED_PSIM_SHIFT
                    | (Lp::SuperSpeedPlus as u32) << PROTO_SPEED_LP_SHIFT
                    | 6 << PROTO_SPEED_PSIV_SHIFT,
            ),
            // SuperSpeedPlus Gen2 x2
            ProtocolSpeed::from_raw(
                (Plt::Symmetric as u32) << PROTO_SPEED_PLT_SHIFT
                    | (true as u32) << PROTO_SPEED_PFD_SHIFT
                    | (Psie::Gbps as u32) << PROTO_SPEED_PSIE_SHIFT
                    | 20 << PROTO_SPEED_PSIM_SHIFT
                    | (Lp::SuperSpeedPlus as u32) << PROTO_SPEED_LP_SHIFT
                    | 7 << PROTO_SPEED_PSIV_SHIFT,
            ),
        ];

        let supp_proto = self.supported_protocol(port)?;

        Some(if supp_proto.psic() != 0 {
            unsafe { supp_proto.protocol_speeds().iter() }
        } else {
            DEFAULT_SUPP_PROTO_SPEEDS.iter()
        })
    }
    pub fn lookup_psiv(&self, port: u8, psiv: u8) -> Option<&'static ProtocolSpeed> {
        self.supported_protocol_speeds(port)?
            .find(|speed| speed.psiv() == psiv)
    }
}
#[derive(Deserialize)]
struct DriverConfig {
    name: String,
    class: u8,
    subclass: i16, // The subclass may be meaningless for some drivers, hence negative values (and values above 255) mean "undefined".
    command: Vec<String>,
}
impl DriverConfig {
    fn subclass(&self) -> Option<u8> {
        u8::try_from(self.subclass).ok()
    }
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

pub(crate) struct IrqFuture {
    state: IrqFutureState,
}

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
                let state = state_weak.upgrade().expect(
                    "IRQ futures keep getting polled even after the driver has been deinitialized",
                );

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
