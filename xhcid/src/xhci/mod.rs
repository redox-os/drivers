//! The eXtensible Host Controller Interface (XHCI) Module
//!
//! This module implements the XHCI functionality of Redox's USB driver daemon.
//!
//! XHCI is a standard for the USB Host Controller interface specified by Intel that provides a
//! common register interface for systems to use to interact with the Universal Serial Bus (USB)
//! subsystem.
//!
//! The standard can be found [here](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf).
//! The standard is referenced frequently throughout this documentation. The acronyms used for specific
//! documents are specified in the crate-level documentation.
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use std::{mem, process, slice, sync::atomic, task, thread};

use common::io::Io;
use syscall::error::{Error, Result, EBADF, EBADMSG, EIO, ENOENT};
use syscall::PAGE_SIZE;

use chashmap::CHashMap;
use common::dma::Dma;
use crossbeam_channel::{Receiver, Sender};
use log::{debug, error, info, trace, warn};
use serde::Deserialize;

use crate::usb;

use pcid_interface::msi::{MsixInfo, MsixTableEntry};
use pcid_interface::{PciFeature, PciFunctionHandle};

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
use self::event::EventRing;
use self::extended::{CapabilityId, ExtendedCapabilitiesIter, ProtocolSpeed, SupportedProtoCap};
use self::irq_reactor::{EventDoorbell, IrqReactor, NewPendingTrb, RingId};
use self::operational::OperationalRegs;
use self::port::Port;
use self::ring::Ring;
use self::runtime::{Interrupter, RuntimeRegs};
use self::trb::{TransferKind, Trb, TrbCompletionCode, TrbType};

use self::scheme::EndpIfState;

use crate::driver_interface::*;

/// Specifies the configurable interrupt mechanism used by the xhci subsystem for registering
/// device state change notifications.
pub enum InterruptMethod {
    /// No interrupts whatsoever; the driver will instead rely on polling event rings.
    Polling,

    /// Legacy PCI INTx# interrupt pin.
    Intx,

    /// Message signaled interrupts.
    Msi,

    /// Extended message signaled interrupts.
    MsiX(Mutex<MappedMsixRegs>),
}

pub struct MappedMsixRegs {
    pub virt_table_base: NonNull<MsixTableEntry>,
    pub info: MsixInfo,
}
impl MappedMsixRegs {
    pub unsafe fn table_entry_pointer_unchecked(&mut self, k: usize) -> &mut MsixTableEntry {
        &mut *self.virt_table_base.as_ptr().offset(k as isize)
    }
    pub fn table_entry_pointer(&mut self, k: usize) -> &mut MsixTableEntry {
        assert!(k < self.info.table_size as usize);
        unsafe { self.table_entry_pointer_unchecked(k) }
    }
}

impl Xhci {
    /// Gets descriptors, before the port state is initiated.
    async fn get_desc_raw<T>(
        &self,
        port: usize,
        slot: u8,
        kind: usb::DescriptorKind,
        index: u8,
        desc: &mut Dma<T>,
    ) -> Result<()> {
        let len = mem::size_of::<T>();
        log::debug!(
            "get_desc_raw port {} slot {} kind {:?} index {} len {}",
            port,
            slot,
            kind,
            index,
            len
        );

        let future = {
            let mut port_state = self.port_states.get_mut(&port).ok_or(Error::new(ENOENT))?;
            let ring = port_state
                .endpoint_states
                .get_mut(&0)
                .ok_or(Error::new(EIO))?
                .ring()
                .expect("no ring for the default control pipe");

            let first_index = ring.next_index();
            let (cmd, cycle) = (&mut ring.trbs[first_index], ring.cycle);
            cmd.setup(
                usb::Setup::get_descriptor(kind, index, 0, len as u16),
                TransferKind::In,
                cycle,
            );

            let (cmd, cycle) = ring.next();
            cmd.data(desc.physical(), len as u16, true, cycle);

            let last_index = ring.next_index();
            let (cmd, cycle) = (&mut ring.trbs[last_index], ring.cycle);

            let interrupter = 0;
            // When the data stage is in, the status stage must be out
            let input = false;
            let ioc = true;
            let ch = false;
            let ent = false;
            cmd.status(interrupter, input, ioc, ch, ent, cycle);

            self.next_transfer_event_trb(
                RingId::default_control_pipe(port as u8),
                &ring,
                &ring.trbs[first_index],
                &ring.trbs[last_index],
                EventDoorbell::new(self, usize::from(slot), Self::def_control_endp_doorbell()),
            )
        };

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let status_trb = trbs.src_trb.unwrap();

        self::scheme::handle_transfer_event_trb("GET_DESC", &event_trb, &status_trb)?;

        self.event_handler_finished();
        Ok(())
    }

    async fn fetch_dev_desc_8_byte(
        &self,
        port: usize,
        slot: u8,
    ) -> Result<usb::DeviceDescriptor8Byte> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<usb::DeviceDescriptor8Byte>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::Device, 0, &mut desc)
            .await?;
        Ok(*desc)
    }

    async fn fetch_dev_desc(&self, port: usize, slot: u8) -> Result<usb::DeviceDescriptor> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<usb::DeviceDescriptor>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::Device, 0, &mut desc)
            .await?;
        Ok(*desc)
    }

    async fn fetch_config_desc(
        &self,
        port: usize,
        slot: u8,
        config: u8,
    ) -> Result<(usb::ConfigDescriptor, [u8; 4087])> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<(usb::ConfigDescriptor, [u8; 4087])>()? };
        self.get_desc_raw(
            port,
            slot,
            usb::DescriptorKind::Configuration,
            config,
            &mut desc,
        )
        .await?;
        Ok(*desc)
    }

    async fn fetch_bos_desc(
        &self,
        port: usize,
        slot: u8,
    ) -> Result<(usb::BosDescriptor, [u8; 4087])> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<(usb::BosDescriptor, [u8; 4087])>()? };
        self.get_desc_raw(
            port,
            slot,
            usb::DescriptorKind::BinaryObjectStorage,
            0,
            &mut desc,
        )
        .await?;
        Ok(*desc)
    }

    async fn fetch_string_desc(&self, port: usize, slot: u8, index: u8) -> Result<String> {
        let mut sdesc = unsafe { self.alloc_dma_zeroed::<(u8, u8, [u16; 127])>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::String, index, &mut sdesc)
            .await?;

        let len = sdesc.0 as usize;
        if len > 2 {
            Ok(String::from_utf16(&sdesc.2[..(len - 2) / 2]).unwrap_or(String::new()))
        } else {
            Ok(String::new())
        }
    }
}

/// The eXtensible Host Controller Interface (XHCI) data structure
pub struct Xhci {
    // immutable
    /// The Host Controller Interface Capability Registers. These read-only registers specify the
    /// limits and capabilities of the host controller implementation (See XHCI section 5.3)
    cap: &'static CapabilityRegs,
    //page_size: usize,

    // XXX: It would be really useful to be able to mutably access individual elements of a slice,
    // without having to wrap every element in a lock (which wouldn't work since they're packed).
    /// The Host Controller Interface Operational Registers. These registers provide the software
    /// interface to configure and monitor the state of the XHCI (See XHCI section 5.4)
    op: Mutex<&'static mut OperationalRegs>,
    ports: Mutex<&'static mut [Port]>,
    /// The Host Controller Interface Doorbell Registers. There is one register per device slot,
    /// and these registers are used by system software to notify the XHC that it has work to perform
    /// for a specific device slot. (See XHCI sections 4.7 and 5.6)
    dbs: Arc<Mutex<&'static mut [Doorbell]>>,
    /// The Host Controller Interface Runtime Registers. These handle interrupt and event processing,
    /// and provide time-sensitive information such as the current microframe. (See XHCI section 5.5)
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
    pcid_handle: Mutex<PciFunctionHandle>,

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
    input_context: Mutex<Dma<InputContext>>,
    dev_desc: Option<DevDesc>,
    endpoint_states: BTreeMap<u8, EndpointState>,
}

impl PortState {
    //TODO: fetch using endpoint number instead
    fn get_endp_desc(&self, endp_idx: u8) -> Option<&EndpDesc> {
        let cfg_idx = self.cfg_idx?;
        let config_desc = self.dev_desc.as_ref()?.config_descs.get(cfg_idx as usize)?;
        let mut endp_count = 0;
        for if_desc in config_desc.interface_descs.iter() {
            for endp_desc in if_desc.endpoints.iter() {
                if endp_idx == endp_count {
                    return Some(endp_desc);
                }
                endp_count += 1;
            }
        }
        None
    }
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
    pub fn new(
        scheme_name: String,
        address: usize,
        interrupt_method: InterruptMethod,
        pcid_handle: PciFunctionHandle,
    ) -> Result<Xhci> {
        //Locate the capability registers from the mapped PCI Bar
        let cap = unsafe { &mut *(address as *mut CapabilityRegs) };
        debug!("CAP REGS BASE {:X}", address);

        //let page_size = ...

        //The operational registers appear immediately after the capability registers.
        let op_base = address + cap.len.read() as usize;
        let op = unsafe { &mut *(op_base as *mut OperationalRegs) };
        debug!("OP REGS BASE {:X}", op_base);

        //Reset the XHCI device
        let (max_slots, max_ports) = {
            debug!("Waiting for xHC becoming ready.");
            // Wait until controller is ready
            while op.usb_sts.readf(1 << 11) {
                trace!("Waiting for the xHC to be ready.");
            }

            debug!("Stopping the xHC");
            // Set run/stop to 0
            op.usb_cmd.writef(1, false);

            debug!("Waiting for the xHC to stop.");
            // Wait until controller not running
            while !op.usb_sts.readf(1) {
                trace!("Waiting for the xHC to stop.");
            }

            debug!("Resetting the xHC.");
            op.usb_cmd.writef(1 << 1, true);
            while op.usb_sts.readf(1 << 1) {
                trace!("Waiting for the xHC to reset.");
            }

            debug!("Reading max slots.");

            let max_slots = cap.max_slots();
            let max_ports = cap.max_ports();

            info!("xHC max slots: {}, max ports: {}", max_slots, max_ports);
            (max_slots, max_ports)
        };

        //Get the address of the port register table
        let port_base = op_base + 0x400;
        let ports =
            unsafe { slice::from_raw_parts_mut(port_base as *mut Port, max_ports as usize) };
        debug!("PORT BASE {:X}", port_base);

        //Get the address of the dorbell register table
        let db_base = address + cap.db_offset.read() as usize;
        let dbs = unsafe { slice::from_raw_parts_mut(db_base as *mut Doorbell, 256) };
        debug!("DOORBELL REGS BASE {:X}", db_base);

        let run_base = address + cap.rts_offset.read() as usize;
        let run = unsafe { &mut *(run_base as *mut RuntimeRegs) };
        debug!("RUNTIME REGS BASE {:X}", run_base);

        // Create the command ring with 4096 / 16 (TRB size) entries, so that it uses all of the
        // DMA allocation (which is at least a 4k page).
        let entries_per_page = PAGE_SIZE / mem::size_of::<Trb>();
        let cmd = Ring::new(cap.ac64(), entries_per_page, true)?;

        let (irq_reactor_sender, irq_reactor_receiver) = crossbeam_channel::unbounded();

        let mut xhci = Self {
            base: address as *const u8,

            cap,
            //page_size,
            op: Mutex::new(op),
            ports: Mutex::new(ports),
            dbs: Arc::new(Mutex::new(dbs)),
            run: Mutex::new(run),

            dev_ctx: DeviceContextList::new(cap.ac64(), max_slots)?,
            scratchpad_buf_arr: None, // initialized in init()

            cmd: Mutex::new(cmd),
            primary_event_ring: Mutex::new(EventRing::new(cap.ac64())?),
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

        xhci.init(max_slots)?;

        Ok(xhci)
    }

    pub fn init(&mut self, max_slots: u8) -> Result<()> {
        // Set run/stop to 0
        debug!("Stopping xHC.");
        self.op.get_mut().unwrap().usb_cmd.writef(1, false);

        // Warm reset
        debug!("Reset xHC");
        self.op.get_mut().unwrap().usb_cmd.writef(1 << 1, true);
        while self.op.get_mut().unwrap().usb_cmd.readf(1 << 1) {
            thread::yield_now();
        }

        // Set enabled slots
        debug!("Setting enabled slots to {}.", max_slots);
        self.op.get_mut().unwrap().config.write(max_slots as u32);
        debug!(
            "Enabled Slots: {}",
            self.op.get_mut().unwrap().config.read() & 0xFF
        );

        // Set device context address array pointer
        let dcbaap = self.dev_ctx.dcbaap();
        debug!("Writing DCBAAP: {:X}", dcbaap);
        self.op.get_mut().unwrap().dcbaap_low.write(dcbaap as u32);
        self.op
            .get_mut()
            .unwrap()
            .dcbaap_high
            .write((dcbaap as u64 >> 32) as u32);

        // Set command ring control register
        let crcr = self.cmd.get_mut().unwrap().register();
        assert_eq!(crcr & 0xFFFF_FFFF_FFFF_FFC1, crcr, "unaligned CRCR");
        debug!("Writing CRCR: {:X}", crcr);
        self.op.get_mut().unwrap().crcr_low.write(crcr as u32);
        self.op
            .get_mut()
            .unwrap()
            .crcr_high
            .write((crcr as u64 >> 32) as u32);

        // Set event ring segment table registers
        debug!(
            "Interrupter 0: {:p}",
            self.run.get_mut().unwrap().ints.as_ptr()
        );
        {
            let int = &mut self.run.get_mut().unwrap().ints[0];

            let erstz = 1;
            debug!("Writing ERSTZ: {}", erstz);
            int.erstsz.write(erstz);

            let erdp = self.primary_event_ring.get_mut().unwrap().erdp();
            debug!("Writing ERDP: {:X}", erdp);
            int.erdp_low.write(erdp as u32 | (1 << 3));
            int.erdp_high.write((erdp as u64 >> 32) as u32);

            let erstba = self.primary_event_ring.get_mut().unwrap().erstba();
            debug!("Writing ERSTBA: {:X}", erstba);
            int.erstba_low.write(erstba as u32);
            int.erstba_high.write((erstba as u64 >> 32) as u32);

            debug!("Writing IMODC and IMODI: {} and {}", 0, 0);
            int.imod.write(0);

            debug!("Enabling Primary Interrupter.");
            int.iman.writef(1 << 1 | 1, true);
        }
        self.op.get_mut().unwrap().usb_cmd.writef(1 << 2, true);

        // Setup the scratchpad buffers that are required for the xHC to function.
        self.setup_scratchpads()?;

        // Set run/stop to 1
        debug!("Starting xHC.");
        self.op.get_mut().unwrap().usb_cmd.writef(1, true);

        // Wait until controller is running
        debug!("Waiting for start request to complete.");
        while self.op.get_mut().unwrap().usb_sts.readf(1) {
            trace!("Waiting for XHCI to report running status.");
        }

        // Ring command doorbell
        debug!("Ringing command doorbell.");
        self.dbs.lock().unwrap()[0].write(0);

        info!("XHCI initialized.");

        self.op.get_mut().unwrap().set_cie(self.cap.cic());

        // Reset ports
        {
            let mut ports = self.ports.lock().unwrap();
            for (i, port) in ports.iter_mut().enumerate() {
                //TODO: only reset if USB 2.0?
                debug!("XHCI Port {} reset", i);

                let instant = std::time::Instant::now();

                port.portsc.writef(port::PortFlags::PORT_PR.bits(), true);
                while port.portsc.readf(port::PortFlags::PORT_PR.bits()) {
                    //while ! port.flags().contains(port::PortFlags::PORT_PRC) {
                    if instant.elapsed().as_secs() >= 1 {
                        warn!("timeout");
                        break;
                    }
                    std::thread::yield_now();
                }
            }
        }

        Ok(())
    }

    pub fn setup_scratchpads(&mut self) -> Result<()> {
        let buf_count = self.cap.max_scratchpad_bufs();

        if buf_count == 0 {
            return Ok(());
        }
        let scratchpad_buf_arr = ScratchpadBufferArray::new(self.cap.ac64(), buf_count)?;
        self.dev_ctx.dcbaa[0] = scratchpad_buf_arr.register() as u64;
        debug!(
            "Setting up {} scratchpads, at {:#0x}",
            buf_count,
            scratchpad_buf_arr.register()
        );
        self.scratchpad_buf_arr = Some(scratchpad_buf_arr);

        Ok(())
    }

    pub async fn enable_port_slot(&self, slot_ty: u8) -> Result<u8> {
        assert_eq!(slot_ty & 0x1F, slot_ty);

        let (event_trb, command_trb) = self
            .execute_command(|cmd, cycle| cmd.enable_slot(slot_ty, cycle))
            .await;

        self::scheme::handle_event_trb("ENABLE_SLOT", &event_trb, &command_trb)?;
        self.event_handler_finished();

        Ok(event_trb.event_slot())
    }
    pub async fn disable_port_slot(&self, slot: u8) -> Result<()> {
        let (event_trb, command_trb) = self
            .execute_command(|cmd, cycle| cmd.disable_slot(slot, cycle))
            .await;

        self::scheme::handle_event_trb("DISABLE_SLOT", &event_trb, &command_trb)?;
        self.event_handler_finished();

        Ok(())
    }

    pub fn slot_state(&self, slot: usize) -> u8 {
        self.dev_ctx.contexts[slot].slot.state()
    }
    pub unsafe fn alloc_dma_zeroed_raw<T>(_ac64: bool) -> Result<Dma<T>> {
        // TODO: ac64
        Ok(Dma::zeroed()?.assume_init())
    }
    pub unsafe fn alloc_dma_zeroed<T>(&self) -> Result<Dma<T>> {
        Self::alloc_dma_zeroed_raw(self.cap.ac64())
    }
    pub unsafe fn alloc_dma_zeroed_unsized_raw<T>(_ac64: bool, count: usize) -> Result<Dma<[T]>> {
        // TODO: ac64
        Ok(Dma::zeroed_slice(count)?.assume_init())
    }
    pub unsafe fn alloc_dma_zeroed_unsized<T>(&self, count: usize) -> Result<Dma<[T]>> {
        Self::alloc_dma_zeroed_unsized_raw(self.cap.ac64(), count)
    }

    pub async fn probe(&self) -> Result<()> {
        debug!(
            "XHCI capabilities: {:?}",
            self.capabilities_iter().collect::<Vec<_>>()
        );

        let port_count = { self.ports.lock().unwrap().len() };

        for i in 0..port_count {
            let (data, state, speed, flags) = {
                let port = &self.ports.lock().unwrap()[i];
                (port.read(), port.state(), port.speed(), port.flags())
            };
            info!(
                "XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}",
                i, data, state, speed, flags
            );

            if flags.contains(port::PortFlags::PORT_CCS) {
                let slot_ty = match self.supported_protocol(i as u8) {
                    Some(protocol) => protocol.proto_slot_ty(),
                    None => {
                        warn!("Failed to find supported protocol information for port");
                        0
                    }
                };

                debug!("Slot type: {}", slot_ty);
                debug!("Enabling slot.");
                let slot = match self.enable_port_slot(slot_ty).await {
                    Ok(ok) => ok,
                    Err(err) => {
                        error!("Failed to enable slot for port {}: {}", i, err);
                        continue;
                    }
                };

                info!("Enabled port {}, which the xHC mapped to {}", i, slot);

                let mut input = unsafe { self.alloc_dma_zeroed::<InputContext>()? };
                let mut ring = match self
                    .address_device(&mut input, i, slot_ty, slot, speed)
                    .await
                {
                    Ok(ok) => ok,
                    Err(err) => {
                        error!("Failed to address device for port {}: {}", i, err);
                        continue;
                    }
                };
                debug!("Addressed device");

                // TODO: Should the descriptors be cached in PortState, or refetched?

                let mut port_state = PortState {
                    slot,
                    input_context: Mutex::new(input),
                    dev_desc: None,
                    cfg_idx: None,
                    endpoint_states: std::iter::once((
                        0,
                        EndpointState {
                            transfer: RingOrStreams::Ring(ring),
                            driver_if_state: EndpIfState::Init,
                        },
                    ))
                    .collect::<BTreeMap<_, _>>(),
                };
                self.port_states.insert(i, port_state);

                // Ensure correct packet size is used
                let dev_desc_8_byte = self.fetch_dev_desc_8_byte(i, slot).await?;
                {
                    let mut port_state = self.port_states.get_mut(&i).unwrap();

                    let mut input = port_state.input_context.lock().unwrap();

                    self.update_max_packet_size(&mut *input, slot, dev_desc_8_byte)
                        .await?;
                }

                let dev_desc = self.get_desc(i, slot).await?;
                self.port_states.get_mut(&i).unwrap().dev_desc = Some(dev_desc);

                {
                    let mut port_state = self.port_states.get_mut(&i).unwrap();

                    let mut input = port_state.input_context.lock().unwrap();
                    let dev_desc = port_state.dev_desc.as_ref().unwrap();

                    self.update_default_control_pipe(&mut *input, slot, dev_desc)
                        .await?;
                }

                match self.spawn_drivers(i) {
                    Ok(()) => (),
                    Err(err) => error!("Failed to spawn driver for port {}: `{}`", i, err),
                }
            }
        }

        Ok(())
    }

    pub async fn update_max_packet_size(
        &self,
        input_context: &mut Dma<InputContext>,
        slot_id: u8,
        dev_desc: usb::DeviceDescriptor8Byte,
    ) -> Result<()> {
        let new_max_packet_size = if dev_desc.major_usb_vers() == 2 {
            u32::from(dev_desc.packet_size)
        } else {
            1u32 << dev_desc.packet_size
        };
        let endp_ctx = &mut input_context.device.endpoints[0];
        let mut b = endp_ctx.b.read();
        b &= 0x0000_FFFF;
        b |= (new_max_packet_size) << 16;
        endp_ctx.b.write(b);

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
            })
            .await;

        self::scheme::handle_event_trb("EVALUATE_CONTEXT", &event_trb, &command_trb)?;
        self.event_handler_finished();

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

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
            })
            .await;

        self::scheme::handle_event_trb("EVALUATE_CONTEXT", &event_trb, &command_trb)?;
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
        let mut ring = Ring::new(self.cap.ac64(), 16, true)?;

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

        let (event_trb, _) = self
            .execute_command(|trb, cycle| {
                trb.address_device(slot, input_context_physical, false, cycle)
            })
            .await;

        if event_trb.completion_code() != TrbCompletionCode::Success as u8 {
            error!(
                "Failed to address device at slot {} (port {}), completion code 0x{:X}",
                slot,
                i,
                event_trb.completion_code()
            );
            self.event_handler_finished();
            return Err(Error::new(EIO));
        }
        self.event_handler_finished();

        Ok(ring)
    }

    pub fn uses_msi(&self) -> bool {
        if let InterruptMethod::Msi = self.interrupt_method {
            true
        } else {
            false
        }
    }
    pub fn uses_msix(&self) -> bool {
        if let InterruptMethod::MsiX(_) = self.interrupt_method {
            true
        } else {
            false
        }
    }
    // TODO: Perhaps use an rwlock?
    pub fn msix_info(&self) -> Option<MutexGuard<'_, MappedMsixRegs>> {
        match self.interrupt_method {
            InterruptMethod::MsiX(ref info) => Some(info.lock().unwrap()),
            _ => None,
        }
    }
    pub fn msix_info_mut(&self) -> Option<MutexGuard<'_, MappedMsixRegs>> {
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
            trace!(
                "Successfully received MSI/MSI-X interrupt, IP={}, EHB={}",
                runtime_regs.ints[0].iman.readf(1),
                runtime_regs.ints[0].erdp_low.readf(3)
            );
            true
        } else if runtime_regs.ints[0].iman.readf(1) {
            trace!(
                "Successfully received INTx# interrupt, IP={}, EHB={}",
                runtime_regs.ints[0].iman.readf(1),
                runtime_regs.ints[0].erdp_low.readf(3)
            );
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
    fn spawn_drivers(&self, port: usize) -> Result<()> {
        // TODO: There should probably be a way to select alternate interfaces, and not just the
        // first one.
        // TODO: Now that there are some good error crates, I don't think errno.h error codes are
        // suitable here.

        let ps = self.port_states.get(&port).unwrap();

        //TODO: support choosing config?
        let config_desc = &ps
            .dev_desc
            .as_ref()
            .ok_or_else(|| {
                log::warn!("Missing device descriptor");
                Error::new(EBADF)
            })?
            .config_descs
            .first()
            .ok_or_else(|| {
                log::warn!("Missing config descriptor");
                Error::new(EBADF)
            })?;

        let drivers_usercfg: &DriversConfig = &DRIVERS_CONFIG;

        //TODO: allow spawning on all interfaces (will require fixing port state)
        for ifdesc in config_desc.interface_descs.iter().take(1) {
            if let Some(driver) = drivers_usercfg.drivers.iter().find(|driver| {
                driver.class == ifdesc.class
                    && driver
                        .subclass()
                        .map(|subclass| subclass == ifdesc.sub_class)
                        .unwrap_or(true)
            }) {
                info!("Loading subdriver \"{}\"", driver.name);
                let (command, args) = driver.command.split_first().ok_or(Error::new(EBADMSG))?;

                let command = if command.starts_with('/') {
                    command.to_owned()
                } else {
                    "/usr/lib/drivers/".to_owned() + command
                };
                let process = process::Command::new(command)
                    .args(
                        args.into_iter()
                            .map(|arg| {
                                arg.replace("$SCHEME", &self.scheme_name)
                                    .replace("$PORT", &format!("{}", port))
                                    .replace("$IF_NUM", &format!("{}", ifdesc.number))
                                    .replace("$IF_PROTO", &format!("{}", ifdesc.protocol))
                            })
                            .collect::<Vec<_>>(),
                    )
                    .stdin(process::Stdio::null())
                    .spawn()
                    .or(Err(Error::new(ENOENT)))?;
                self.drivers.insert(port, process);
            } else {
                warn!(
                    "No driver for USB class {}.{}",
                    ifdesc.class, ifdesc.sub_class
                );
            }
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
    ) -> impl Iterator<Item = &'static ProtocolSpeed> {
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

        match self.supported_protocol(port) {
            Some(supp_proto) => {
                if supp_proto.psic() != 0 {
                    unsafe { supp_proto.protocol_speeds().iter() }
                } else {
                    DEFAULT_SUPP_PROTO_SPEEDS.iter()
                }
            }
            None => {
                log::warn!(
                    "falling back to default supported protocol speeds for port {}",
                    port
                );
                DEFAULT_SUPP_PROTO_SPEEDS.iter()
            }
        }
    }
    pub fn lookup_psiv(&self, port: u8, psiv: u8) -> Option<&'static ProtocolSpeed> {
        self.supported_protocol_speeds(port)
            .find(|speed| speed.psiv() == psiv)
    }
}
pub fn start_irq_reactor(hci: &Arc<Xhci>, irq_file: Option<File>) {
    let receiver = hci.irq_reactor_receiver.clone();
    let hci_clone = Arc::clone(&hci);

    debug!("About to start IRQ reactor");

    *hci.irq_reactor.lock().unwrap() = Some(thread::spawn(move || {
        debug!("Started IRQ reactor thread");
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
