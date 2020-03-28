use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::sync::atomic::{AtomicBool, AtomicUsize};

use std::{mem, process, slice, sync::atomic, task, thread};

use chashmap::CHashMap;
use crossbeam_channel::{Receiver, Sender};
use serde::Deserialize;
use syscall::error::{Error, Result, EBADF, EBADMSG, ENOENT};
use syscall::flag::O_RDONLY;
use syscall::io::{Dma, Io};

use crate::usb;

use pcid_interface::msi::{MsixTableEntry, MsixCapability};
use pcid_interface::{PcidServerHandle, PciFeature};

mod capability;
mod context;
mod doorbell;
mod event;
mod extended;
pub mod irq_reactor;
mod operational;
mod port;
mod ring;
mod runtime;
pub mod scheme;
mod trb;

use self::capability::CapabilityRegs;
use self::context::{DeviceContextList, InputContext, ScratchpadBufferArray, StreamContextArray};
use self::doorbell::Doorbell;
use self::irq_reactor::{IrqReactor, NewPendingTrb, RingId};
use self::event::EventRing;
use self::extended::{CapabilityId, ExtendedCapabilitiesIter, ProtocolSpeed, SupportedProtoCap};
use self::operational::OperationalRegs;
use self::port::Port;
use self::ring::Ring;
use self::runtime::{Interrupter, RuntimeRegs};
use self::trb::{TransferKind, Trb, TrbCompletionCode, TrbType};

use self::scheme::EndpIfState;

use crate::driver_interface::*;

pub enum InterruptMethod {
    /// No interrupts whatsoever; the driver will instead rely on polling event rings.
    Polling,

    /// Legacy PCI INTx# interrupt pin.
    Intx,

    /// Message signaled interrupts.
    Msi,

    /// Extended message signaled interrupts.
    MsiX(Mutex<MsixInfo>),
}

pub struct MsixInfo {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub virt_pba_base: NonNull<u64>,
    pub capability: MsixCapability,
}
impl MsixInfo {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().offset(k as isize)
    }
    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
    pub unsafe fn pba_pointer_unchecked(&mut self, k: usize) -> &mut u64 {
        &mut *self.virt_pba_base.as_ptr().offset(k as isize)
    }
    pub fn pba_pointer(&mut self, k: usize) -> &mut u64 {
        assert!(k < self.capability.table_size() as usize);
        unsafe { self.pba_pointer_unchecked(k) }
    }
    pub fn pba(&mut self, k: usize) -> bool {
        let byte = k / 64;
        let bit = k % 64;
        *self.pba_pointer(byte) & (1 << bit) != 0
    }
}

impl Xhci {
    /// Gets descriptors, before the port state is initiated.
    async fn get_desc_raw<T>(&self, port: usize, slot: u8, kind: usb::DescriptorKind, index: u8, ring: &mut Ring, desc: &mut Dma<T>) -> Result<()> {
        let len = mem::size_of::<T>();

        let future = {
            let (cmd, cycle) = ring.next();
            cmd.setup(
                usb::Setup::get_descriptor(kind, index, 0, len as u16),
                TransferKind::In,
                cycle,
            );

            let (cmd, cycle) = ring.next();
            cmd.data(desc.physical(), len as u16, true, cycle);

            let (cmd, cycle) = ring.next();
            cmd.status(0, true, true, false, false, cycle);

            self.next_transfer_event_trb(RingId::default_control_pipe(port as u8), &cmd)
        };

        self.dbs.lock().unwrap()[usize::from(slot)].write(Self::def_control_endp_doorbell());

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let status_trb = trbs.src_trb.unwrap();

        self::scheme::handle_transfer_event_trb("GET_DESC", &event_trb, &status_trb)?;

        self.event_handler_finished();
        Ok(())
    }

    async fn fetch_dev_desc(&self, port: usize, slot: u8, ring: &mut Ring) -> Result<usb::DeviceDescriptor> {
        let mut desc = Dma::<usb::DeviceDescriptor>::zeroed()?;
        self.get_desc_raw(port, slot, usb::DescriptorKind::Device, 0, ring, &mut desc).await?;
        Ok(*desc)
    }

    async fn fetch_config_desc(&self, port: usize, slot: u8, ring: &mut Ring, config: u8) -> Result<(usb::ConfigDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::ConfigDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc_raw(port, slot, usb::DescriptorKind::Configuration, config, ring, &mut desc).await?;
        Ok(*desc)
    }

    async fn fetch_bos_desc(&self, port: usize, slot: u8, ring: &mut Ring) -> Result<(usb::BosDescriptor, [u8; 4087])> {
        let mut desc = Dma::<(usb::BosDescriptor, [u8; 4087])>::zeroed()?;
        self.get_desc_raw(port, slot, usb::DescriptorKind::BinaryObjectStorage, 0, ring, &mut desc).await?;
        Ok(*desc)
    }

    async fn fetch_string_desc(&self, port: usize, slot: u8, ring: &mut Ring, index: u8) -> Result<String> {
        let mut sdesc = Dma::<(u8, u8, [u16; 127])>::zeroed()?;
        self.get_desc_raw(port, slot, usb::DescriptorKind::String, index, ring, &mut sdesc).await?;

        let len = sdesc.0 as usize;
        if len > 2 {
            Ok(String::from_utf16(&sdesc.2[..(len - 2) / 2]).unwrap_or(String::new()))
        } else {
            Ok(String::new())
        }
    }
}

pub struct Xhci {
    // immutable
    cap: &'static CapabilityRegs,
    page_size: usize,

    // XXX: It would be really useful to be able to mutably access individual elements of a slice,
    // without having to wrap every element in a lock (which wouldn't work since they're packed).
    op: Mutex<&'static mut OperationalRegs>,
    ports: Mutex<&'static mut [Port]>,
    dbs: Mutex<&'static mut [Doorbell]>,
    run: Mutex<&'static mut RuntimeRegs>,
    cmd: Mutex<Ring>,
    primary_event_ring: Mutex<EventRing>,

    // immutable
    dev_ctx: DeviceContextList,
    scratchpad_buf_arr: Option<ScratchpadBufferArray>,

    // used for the extended capabilities, and so far none of them are mutated, and thus no lock.
    base: *const u8,

    handles: CHashMap<usize, scheme::Handle>,
    next_handle: AtomicUsize,
    port_states: CHashMap<usize, PortState>,

    drivers: CHashMap<usize, process::Child>,
    scheme_name: String,

    interrupt_method: InterruptMethod,
    pcid_handle: Mutex<PcidServerHandle>,

    irq_reactor: Mutex<Option<thread::JoinHandle<()>>>,

    irq_reactor_sender: Sender<NewPendingTrb>,

    // not used, but still stored so that the thread, when created, can get the channel without the
    // channel being in a mutex.
    irq_reactor_receiver: Receiver<NewPendingTrb>,
}

unsafe impl Send for Xhci {}
unsafe impl Sync for Xhci {}

struct PortState {
    slot: u8,
    cfg_idx: Option<u8>,
    if_idx: Option<u8>,
    input_context: Mutex<Dma<InputContext>>,
    dev_desc: Option<DevDesc>,
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
    pub fn new(scheme_name: String, address: usize, interrupt_method: InterruptMethod, pcid_handle: PcidServerHandle) -> Result<Xhci> {
        let cap = unsafe { &mut *(address as *mut CapabilityRegs) };
        println!("  - CAP {:X}", address);

        let page_size = {
            let memory_fd = syscall::open("memory:", O_RDONLY)?;
            let mut stat = syscall::data::StatVfs::default();
            syscall::fstatvfs(memory_fd, &mut stat)?;
            stat.f_bsize as usize
        };

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

        // Create the command ring with 4096 / 16 (TRB size) entries, so that it uses all of the
        // DMA allocation (which is at least a 4k page).
        let entries_per_page = page_size / mem::size_of::<Trb>();
        let cmd = Ring::new(entries_per_page, true)?;

        let (irq_reactor_sender, irq_reactor_receiver) = crossbeam_channel::unbounded();

        let mut xhci = Self {
            base: address as *const u8,

            cap,
            page_size,

            op: Mutex::new(op),
            ports: Mutex::new(ports),
            dbs: Mutex::new(dbs),
            run: Mutex::new(run),

            dev_ctx: DeviceContextList::new(max_slots)?,
            scratchpad_buf_arr: None, // initialized in init()

            cmd: Mutex::new(cmd),
            primary_event_ring: Mutex::new(EventRing::new()?),
            handles: CHashMap::new(),
            next_handle: AtomicUsize::new(0),
            port_states: CHashMap::new(),

            drivers: CHashMap::new(),
            scheme_name,

            interrupt_method,
            pcid_handle: Mutex::new(pcid_handle),

            irq_reactor: Mutex::new(None),
            irq_reactor_sender,
            irq_reactor_receiver,
        };

        xhci.init(max_slots);

        Ok(xhci)
    }

    pub fn init(&mut self, max_slots: u8) -> Result<()> {
        // Set enabled slots
        println!("  - Set enabled slots to {}", max_slots);
        self.op.get_mut().unwrap().config.write(max_slots as u32);
        println!("  - Enabled Slots: {}", self.op.get_mut().unwrap().config.read() & 0xFF);

        // Set device context address array pointer
        let dcbaap = self.dev_ctx.dcbaap();
        println!("  - Write DCBAAP: {:X}", dcbaap);
        self.op.get_mut().unwrap().dcbaap.write(dcbaap as u64);

        // Set command ring control register
        let crcr = self.cmd.get_mut().unwrap().register();
        assert_eq!(crcr & 0xFFFF_FFFF_FFFF_FFC1, crcr, "unaligned CRCR");
        println!("  - Write CRCR: {:X}", crcr);
        self.op.get_mut().unwrap().crcr.write(crcr as u64);

        // Set event ring segment table registers
        println!("  - Interrupter 0: {:X}", self.run.get_mut().unwrap().ints.as_ptr() as usize);
        {
            let int = &mut self.run.get_mut().unwrap().ints[0];

            let erstz = 1;
            println!("  - Write ERSTZ: {}", erstz);
            int.erstsz.write(erstz);

            let erdp = self.primary_event_ring.get_mut().unwrap().erdp();
            println!("  - Write ERDP: {:X}", erdp);
            int.erdp.write(erdp as u64 | (1 << 3));

            let erstba = self.primary_event_ring.get_mut().unwrap().erstba();
            println!("  - Write ERSTBA: {:X}", erstba);
            int.erstba.write(erstba as u64);

            println!("  - Write IMODC and IMODI: {} and {}", 0, 0);
            int.imod.write(0);

            println!("  - Enable interrupts");
            int.iman.writef(1 << 1 | 1, true);

        }
        self.op.get_mut().unwrap().usb_cmd.writef(1 << 2, true);

        // Setup the scratchpad buffers that are required for the xHC to function.
        self.setup_scratchpads()?;

        // Set run/stop to 1
        println!("  - Start");
        self.op.get_mut().unwrap().usb_cmd.writef(1, true);

        // Wait until controller is running
        println!("  - Wait for running");
        while self.op.get_mut().unwrap().usb_sts.readf(1) {
            println!("  - Waiting for XHCI running");
        }

        println!("IP={}", self.run.get_mut().unwrap().ints[0].iman.readf(1));

        // Ring command doorbell
        println!("  - Ring doorbell");
        self.dbs.get_mut().unwrap()[0].write(0);

        println!("  - XHCI initialized");

        if self.cap.cic() {
            self.op.get_mut().unwrap().set_cie(true);
        }

        Ok(())
    }

    pub fn setup_scratchpads(&mut self) -> Result<()> {
        let buf_count = self.cap.max_scratchpad_bufs();

        if buf_count == 0 {
            return Ok(());
        }
        let scratchpad_buf_arr = ScratchpadBufferArray::new(self.page_size,buf_count)?;
        self.dev_ctx.dcbaa[0] = scratchpad_buf_arr.register() as u64;
        self.scratchpad_buf_arr = Some(scratchpad_buf_arr);

        Ok(())
    }

    pub async fn enable_port_slot(&self, slot_ty: u8) -> Result<u8> {
        assert_eq!(slot_ty & 0x1F, slot_ty);

        let (event_trb, command_trb) =
            self.execute_command(|cmd, cycle| cmd.enable_slot(slot_ty, cycle)).await;

        self::scheme::handle_event_trb("ENABLE_SLOT", &event_trb, &command_trb);
        self.event_handler_finished();

        Ok(event_trb.event_slot())
    }
    pub async fn disable_port_slot(&self, slot: u8) -> Result<()> {
        let (event_trb, command_trb) = self.execute_command(|cmd, cycle| cmd.disable_slot(slot, cycle)).await;

        self::scheme::handle_event_trb("DISABLE_SLOT", &event_trb, &command_trb);
        self.event_handler_finished();

        Ok(())
    }

    pub fn slot_state(&self, slot: usize) -> u8 {
        self.dev_ctx.contexts[slot].slot.state()
    }

    pub async fn probe(&self) -> Result<()> {
        println!("XHCI capabilities: {:?}", self.capabilities_iter().collect::<Vec<_>>());

        let port_count = { self.ports.lock().unwrap().len() };

        for i in 0..port_count {
            let (data, state, speed, flags) = {
                let port = &self.ports.lock().unwrap()[i];
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

                println!("Got slot type: {}", slot_ty);
                let slot = self.enable_port_slot(slot_ty).await?;

                println!("    - Slot {}", slot);

                let mut input = Dma::<InputContext>::zeroed()?;
                let mut ring = self.address_device(&mut input, i, slot_ty, slot, speed).await?;

                // TODO: Should the descriptors be cached in PortState, or refetched?

                let mut port_state = PortState {
                    slot,
                    input_context: Mutex::new(input),
                    dev_desc: None,
                    cfg_idx: None,
                    if_idx: None,
                    endpoint_states: std::iter::once((
                        0,
                        EndpointState {
                            transfer: RingOrStreams::Ring(ring),
                            driver_if_state: EndpIfState::Init,
                        },
                    ))
                    .collect::<BTreeMap<_, _>>(),
                };

                let ring = port_state.endpoint_states.get_mut(&0).unwrap().ring().unwrap();

                let dev_desc = self.get_desc(i, slot, ring).await?;
                port_state.dev_desc = Some(dev_desc);


                {
                    let mut input = port_state.input_context.lock().unwrap();
                    let dev_desc = port_state.dev_desc.as_ref().unwrap();

                    self.update_default_control_pipe(&mut *input, slot, dev_desc).await?;
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

    pub async fn update_default_control_pipe(
        &self,
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

        let (event_trb, command_trb) = self.execute_command(|trb, cycle| {
            trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
        }).await;

        self::scheme::handle_event_trb("EVALUATE_CONTEXT", &event_trb, &command_trb);
        self.event_handler_finished();

        Ok(())
    }

    pub async fn address_device(
        &self,
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

        let (event_trb, _) = self.execute_command(|trb, cycle| {
            trb.address_device(slot, input_context_physical, false, cycle)
        }).await;

        if event_trb.completion_code() != TrbCompletionCode::Success as u8 {
            println!("Failed to address device at slot {} (port {})", slot, i);
        }

        Ok(ring)
    }

    pub fn uses_msi(&self) -> bool {
        if let InterruptMethod::Msi = self.interrupt_method { true } else { false }
    }
    pub fn uses_msix(&self) -> bool {
        if let InterruptMethod::MsiX(_) = self.interrupt_method { true } else { false }
    }
    // TODO: Perhaps use an rwlock?
    pub fn msix_info(&self) -> Option<MutexGuard<'_, MsixInfo>> {
        match self.interrupt_method {
            InterruptMethod::MsiX(ref info) => Some(info.lock().unwrap()),
            _ => None,
        }
    }
    pub fn msix_info_mut(&self) -> Option<MutexGuard<'_, MsixInfo>> {
        match self.interrupt_method {
            InterruptMethod::MsiX(ref info) => Some(info.lock().unwrap()),
            _ => None,
        }
    }

    /// Checks whether an IRQ has been received from *this* device, in case of an interrupt. Always
    /// true when using MSI/MSI-X.
    pub fn received_irq(&self) -> bool {
        let mut runtime_regs = self.run.lock().unwrap();

        if self.uses_msi() || self.uses_msix() {
            // Since using MSI and MSI-X implies having no IRQ sharing whatsoever, the IP bit
            // doesn't have to be touched.
            println!("Successfully received MSI/MSI-X interrupt, IP={}, EHB={}", runtime_regs.ints[0].iman.readf(1), runtime_regs.ints[0].erdp.readf(3));
            println!("MSI-X PB={}", self.msix_info_mut().unwrap().pba(0));
            let mut msix = self.msix_info_mut().unwrap();
            let entry = msix.table_entry_pointer(0);
            println!("MSI-X entry (addr_lo, addr_hi, msg_data, vec_ctl: {:#0x} {:#0x} {:#0x} {:#0x}", entry.addr_lo.read(), entry.addr_hi.read(), entry.msg_data.read(), entry.vec_ctl.read());
            true
        } else if runtime_regs.ints[0].iman.readf(1) {
            // If MSI and/or MSI-X are not used, the interrupt might have to be shared, and thus there is
            // a special register to specify whether the IRQ actually came from the xHC.
            runtime_regs.ints[0].iman.writef(1, true);

            // The interrupt came from the xHC.
            true
        } else {
            // The interrupt came from a different device.
            false
        }

    }
    fn spawn_drivers(&self, port: usize, ps: &mut PortState) -> Result<()> {
        // TODO: There should probably be a way to select alternate interfaces, and not just the
        // first one.
        // TODO: Now that there are some good error crates, I don't think errno.h error codes are
        // suitable here.

        let ifdesc = &ps
            .dev_desc
            .as_ref().unwrap()
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
pub fn start_irq_reactor(hci: &Arc<Xhci>, irq_file: Option<File>) {
    let receiver = hci.irq_reactor_receiver.clone();
    let hci_clone = Arc::clone(&hci);

    println!("About to start IRQ reactor");

    *hci.irq_reactor.lock().unwrap() = Some(thread::spawn(move || {
        println!("Started IRQ reactor thread");
        IrqReactor::new(hci_clone, receiver, irq_file).run()
    }));
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
