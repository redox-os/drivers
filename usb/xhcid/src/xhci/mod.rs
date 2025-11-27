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
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use std::{mem, process, slice, thread};
use syscall::error::{Error, Result, EBADF, EBADMSG, EIO, ENOENT};
use syscall::{EAGAIN, PAGE_SIZE};

use chashmap::CHashMap;
use common::{dma::Dma, io::Io, timeout::Timeout};
use crossbeam_channel::{Receiver, Sender};
use log::{debug, error, info, trace, warn};
use serde::Deserialize;

use crate::usb;

use pcid_interface::PciFunctionHandle;

mod capability;
mod context;
mod device_enumerator;
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

pub use self::capability::CapabilityRegs;
use self::context::{
    DeviceContextList, InputContext, ScratchpadBufferArray, StreamContextArray,
    SLOT_CONTEXT_STATE_MASK, SLOT_CONTEXT_STATE_SHIFT,
};
pub use self::context::{CONTEXT_32, CONTEXT_64};
use self::doorbell::Doorbell;
use self::event::EventRing;
use self::extended::{CapabilityId, ExtendedCapabilitiesIter, ProtocolSpeed, SupportedProtoCap};
use self::irq_reactor::{EventDoorbell, IrqReactor, NewPendingTrb, RingId};
use self::operational::*;
use self::port::Port;
use self::ring::Ring;
use self::runtime::RuntimeRegs;
use self::trb::{TransferKind, Trb, TrbCompletionCode};

use self::scheme::EndpIfState;

pub use crate::driver_interface::PortId;
use crate::driver_interface::*;

/// Specifies the configurable interrupt mechanism used by the xhci subsystem for registering
/// device state change notifications.
pub enum InterruptMethod {
    /// No interrupts whatsoever; the driver will instead rely on polling event rings.
    Polling,

    /// Legacy PCI INTx# interrupt pin.
    Intx,

    /// (Extended) Message signaled interrupts.
    Msi,
}

impl<const N: usize> Xhci<N> {
    /// Gets descriptors, before the port state is initiated.
    async fn get_desc_raw<T>(
        &self,
        port: PortId,
        slot: u8,
        kind: usb::DescriptorKind,
        value: u8,
        index: u16,
        desc: &mut Dma<T>,
    ) -> Result<()> {
        if self.interrupt_is_pending(0) {
            debug!("EHB is already set!");
            self.force_clear_interrupt(0);
        }
        let len = mem::size_of::<T>();
        log::debug!(
            "get_desc_raw port {} slot {} kind {:?} value {} index {} len {}",
            port,
            slot,
            kind,
            value,
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
                usb::Setup::get_descriptor(kind, value, index, len as u16),
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
                RingId::default_control_pipe(port),
                &ring,
                &ring.trbs[first_index],
                &ring.trbs[last_index],
                EventDoorbell::new(self, usize::from(slot), Self::def_control_endp_doorbell()),
            )
        };

        debug!("Waiting for the next transfer event TRB...");
        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let status_trb = trbs.src_trb.ok_or(Error::new(EIO))?;
        trace!("Handling the transfer event TRB!");
        self::scheme::handle_transfer_event_trb("GET_DESC", &event_trb, &status_trb)?;

        //self.event_handler_finished();
        Ok(())
    }

    async fn fetch_dev_desc_8_byte(
        &self,
        port: PortId,
        slot: u8,
    ) -> Result<usb::DeviceDescriptor8Byte> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<usb::DeviceDescriptor8Byte>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::Device, 0, 0, &mut desc)
            .await?;
        Ok(*desc)
    }

    async fn fetch_dev_desc(&self, port: PortId, slot: u8) -> Result<usb::DeviceDescriptor> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<usb::DeviceDescriptor>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::Device, 0, 0, &mut desc)
            .await?;
        Ok(*desc)
    }

    async fn fetch_config_desc(
        &self,
        port: PortId,
        slot: u8,
        config: u8,
    ) -> Result<(usb::ConfigDescriptor, [u8; 4087])> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<(usb::ConfigDescriptor, [u8; 4087])>()? };
        self.get_desc_raw(
            port,
            slot,
            usb::DescriptorKind::Configuration,
            config,
            0,
            &mut desc,
        )
        .await?;
        Ok(*desc)
    }

    async fn fetch_bos_desc(
        &self,
        port: PortId,
        slot: u8,
    ) -> Result<(usb::BosDescriptor, [u8; 4087])> {
        let mut desc = unsafe { self.alloc_dma_zeroed::<(usb::BosDescriptor, [u8; 4087])>()? };
        self.get_desc_raw(
            port,
            slot,
            usb::DescriptorKind::BinaryObjectStorage,
            0,
            0,
            &mut desc,
        )
        .await?;
        Ok(*desc)
    }

    async fn fetch_lang_ids_desc(&self, port: PortId, slot: u8) -> Result<Vec<u16>> {
        let mut sdesc = unsafe { self.alloc_dma_zeroed::<(u8, u8, [u16; 127])>()? };
        self.get_desc_raw(port, slot, usb::DescriptorKind::String, 0, 0, &mut sdesc)
            .await?;

        let len = sdesc.0 as usize;
        if len > 2 {
            Ok(sdesc.2[..(len - 2) / 2].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_string_desc(
        &self,
        port: PortId,
        slot: u8,
        value: u8,
        lang_id: u16,
    ) -> Result<String> {
        let mut sdesc = unsafe { self.alloc_dma_zeroed::<(u8, u8, [u16; 127])>()? };
        self.get_desc_raw(
            port,
            slot,
            usb::DescriptorKind::String,
            value,
            lang_id,
            &mut sdesc,
        )
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
pub struct Xhci<const N: usize> {
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
    dev_ctx: DeviceContextList<N>,
    scratchpad_buf_arr: Option<ScratchpadBufferArray>,

    // used for the extended capabilities, and so far none of them are mutated, and thus no lock.
    base: *const u8,

    handles: CHashMap<usize, scheme::Handle>,
    next_handle: AtomicUsize,
    port_states: CHashMap<PortId, PortState<N>>,
    drivers: CHashMap<PortId, Vec<process::Child>>,
    scheme_name: String,

    interrupt_method: InterruptMethod,
    pcid_handle: Mutex<PciFunctionHandle>,

    irq_reactor: Mutex<Option<thread::JoinHandle<()>>>,

    irq_reactor_sender: Sender<NewPendingTrb>,

    // not used, but still stored so that the thread, when created, can get the channel without the
    // channel being in a mutex.
    irq_reactor_receiver: Receiver<NewPendingTrb>,
    device_enumerator: Mutex<Option<thread::JoinHandle<()>>>,
    device_enumerator_sender: Sender<DeviceEnumerationRequest>,
    device_enumerator_receiver: Receiver<DeviceEnumerationRequest>,
}

unsafe impl<const N: usize> Send for Xhci<N> {}
unsafe impl<const N: usize> Sync for Xhci<N> {}

struct PortState<const N: usize> {
    slot: u8,
    protocol_speed: &'static ProtocolSpeed,
    cfg_idx: Option<u8>,
    input_context: Mutex<Dma<InputContext<N>>>,
    dev_desc: Option<DevDesc>,
    endpoint_states: BTreeMap<u8, EndpointState>,
}

impl<const N: usize> PortState<N> {
    //TODO: fetch using endpoint number instead
    fn get_endp_desc(&self, endp_idx: u8) -> Option<&EndpDesc> {
        let cfg_idx = self.cfg_idx?;
        let config_desc = self
            .dev_desc
            .as_ref()?
            .config_descs
            .iter()
            .find(|desc| desc.configuration_value == cfg_idx)?;
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

impl<const N: usize> Xhci<N> {
    pub fn new(
        scheme_name: String,
        address: usize,
        interrupt_method: InterruptMethod,
        pcid_handle: PciFunctionHandle,
    ) -> Result<Self> {
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
            {
                debug!("Waiting for xHC becoming ready.");
                let timeout = Timeout::from_secs(1);
                while op.usb_sts.readf(USB_STS_CNR) {
                    timeout.run().map_err(|()| {
                        log::error!("timeout on USB_STS_CNR");
                        Error::new(EIO)
                    })?;
                }
            }

            debug!("Stopping the xHC");
            // Set run/stop to 0
            op.usb_cmd.writef(USB_CMD_RS, false);

            {
                debug!("Waiting for the xHC to stop.");
                let timeout = Timeout::from_secs(1);
                while !op.usb_sts.readf(USB_STS_HCH) {
                    timeout.run().map_err(|()| {
                        log::error!("timeout on USB_STS_HCH");
                        Error::new(EIO)
                    })?;
                }
            }

            {
                debug!("Resetting the xHC.");
                op.usb_cmd.writef(USB_CMD_HCRST, true);
                let timeout = Timeout::from_secs(1);
                while op.usb_cmd.readf(USB_CMD_HCRST) {
                    timeout.run().map_err(|()| {
                        log::error!("timeout on USB_CMD_HCRST");
                        Error::new(EIO)
                    })?;
                }
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
        let cmd = Ring::new::<N>(cap.ac64(), entries_per_page, true)?;

        let (irq_reactor_sender, irq_reactor_receiver) = crossbeam_channel::unbounded();

        let (device_enumerator_sender, device_enumerator_receiver) = crossbeam_channel::unbounded();

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
            primary_event_ring: Mutex::new(EventRing::new::<N>(cap.ac64())?),
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
            device_enumerator: Mutex::new(None),
            device_enumerator_sender,
            device_enumerator_receiver,
        };

        xhci.init(max_slots)?;

        Ok(xhci)
    }

    pub fn init(&mut self, max_slots: u8) -> Result<()> {
        // Set run/stop to 0
        debug!("Stopping xHC.");
        self.op.get_mut().unwrap().usb_cmd.writef(USB_CMD_RS, false);

        // Warm reset
        {
            debug!("Reset xHC");
            let timeout = Timeout::from_secs(1);
            self.op
                .get_mut()
                .unwrap()
                .usb_cmd
                .writef(USB_CMD_HCRST, true);
            while self.op.get_mut().unwrap().usb_cmd.readf(USB_CMD_HCRST) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on USB_CMD_HCRST");
                    Error::new(EIO)
                })?;
            }
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
        self.op
            .get_mut()
            .unwrap()
            .usb_cmd
            .writef(USB_CMD_INTE, true);

        // Setup the scratchpad buffers that are required for the xHC to function.
        self.setup_scratchpads()?;

        // Set run/stop to 1
        debug!("Starting xHC.");
        self.op.get_mut().unwrap().usb_cmd.writef(USB_CMD_RS, true);

        {
            debug!("Waiting for start request to complete.");
            let timeout = Timeout::from_secs(1);
            while self.op.get_mut().unwrap().usb_sts.readf(USB_STS_HCH) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on USB_STS_HCH");
                    Error::new(EIO)
                })?;
            }
        }

        // Ring command doorbell
        debug!("Ringing command doorbell.");
        self.dbs.lock().unwrap()[0].write(0);

        info!("XHCI initialized.");

        self.op.get_mut().unwrap().set_cie(self.cap.cic());

        self.print_port_capabilities();

        Ok(())
    }

    pub fn get_pls(&self, port_id: PortId) -> u8 {
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port_id.root_hub_port_index()).unwrap();
        port.state()
    }

    pub fn poll(&self) {
        debug!("Polling Initial Devices!");

        let len = self.ports.lock().unwrap().len();

        for root_hub_port_num in 1..=(len as u8) {
            let port_id = PortId {
                root_hub_port_num,
                route_string: 0,
            };

            //Get the CCS and CSC flags
            let (ccs, csc, flags) = {
                let mut ports = self.ports.lock().unwrap();
                let port = &mut ports[port_id.root_hub_port_index()];
                let flags = port.flags();
                let ccs = flags.contains(PortFlags::CCS);
                let csc = flags.contains(PortFlags::CSC);

                (ccs, csc, flags)
            };

            debug!("Port {} has flags {:?}", port_id, flags);

            match (ccs, csc) {
                (false, false) => { // Nothing is connected, and there was no port status change
                     //Do nothing
                }
                _ => {
                    //Either something is connected, or nothing is connected and a port status change was asserted.
                    self.device_enumerator_sender
                        .send(DeviceEnumerationRequest { port_id })
                        .expect("Failed to generate the port enumeration request!");
                }
            }
        }
    }

    pub fn print_port_capabilities(&self) {
        let len;
        {
            let mut ports = self.ports.lock().unwrap();
            len = ports.len();
        }

        for root_hub_port_num in 1..=(len as u8) {
            let port_id = PortId {
                root_hub_port_num,
                route_string: 0,
            };

            let state = self.get_pls(port_id);
            let mut flags;
            {
                let mut ports = self.ports.lock().unwrap();

                flags = ports[port_id.root_hub_port_index()].flags();
            }

            match self.supported_protocol(port_id) {
                None => {
                    warn!("No detected supported protocol for port {}", port_id);
                }
                Some(protocol) => {
                    info!(
                        "Port {} is a USB {}.{} port with slot type {} and in current state {}: {:?}",
                        port_id,
                        protocol.rev_major(),
                        protocol.rev_minor(),
                        protocol.proto_slot_ty(),
                        state,
                        flags
                    );
                }
            };
        }
    }
    pub fn reset_port(&self, port_id: PortId) -> Result<()> {
        debug!("XHCI Port {} reset", port_id);

        //TODO handle the second unwrap
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port_id.root_hub_port_index()).unwrap();
        let instant = std::time::Instant::now();

        debug!("Port {} Link State: {}", port_id, port.state());

        {
            port.set_pr();
            debug!(
                "Flags after setting port {} reset: {:?}",
                port_id,
                port.flags()
            );
            let timeout = Timeout::from_secs(1);
            while !port.flags().contains(port::PortFlags::PRC) {
                timeout.run().map_err(|()| {
                    log::error!("timeout on port {} PRC", port_id);
                    Error::new(EIO)
                })?;
            }
        }
        Ok(())
    }

    pub fn setup_scratchpads(&mut self) -> Result<()> {
        let buf_count = self.cap.max_scratchpad_bufs();

        if buf_count == 0 {
            return Ok(());
        }
        let scratchpad_buf_arr = ScratchpadBufferArray::new::<N>(self.cap.ac64(), buf_count)?;
        self.dev_ctx.dcbaa[0] = scratchpad_buf_arr.register() as u64;
        debug!(
            "Setting up {} scratchpads, at {:#0x}",
            buf_count,
            scratchpad_buf_arr.register()
        );
        self.scratchpad_buf_arr = Some(scratchpad_buf_arr);

        Ok(())
    }

    pub fn force_clear_interrupt(&self, index: usize) {
        {
            // If ERDP EHB bit is set, clear it before sending command
            //TODO: find out why this bit is set earlier!
            let mut run = self.run.lock().unwrap();
            let mut int = &mut run.ints[index];

            if int.erdp_low.readf(1 << 3) {
                int.erdp_low.writef(1 << 3, true);
            } else {
                warn!("Attempted to clear the interrupt bit when no interrupt was pending");
            }
        }
    }

    pub fn interrupt_is_pending(&self, index: usize) -> bool {
        let mut run = self.run.lock().unwrap();
        let mut int = &mut run.ints[index];
        int.erdp_low.readf(1 << 3)
    }

    pub async fn enable_port_slot(&self, slot_ty: u8) -> Result<u8> {
        assert_eq!(slot_ty & 0x1F, slot_ty);

        let (event_trb, command_trb) = self
            .execute_command(|cmd, cycle| cmd.enable_slot(slot_ty, cycle))
            .await;

        trace!("Slot is enabled!");
        self::scheme::handle_event_trb("ENABLE_SLOT", &event_trb, &command_trb)?;
        //self.event_handler_finished();

        Ok(event_trb.event_slot())
    }
    pub async fn disable_port_slot(&self, slot: u8) -> Result<()> {
        trace!("Disable slot {}", slot);
        let (event_trb, command_trb) = self
            .execute_command(|cmd, cycle| cmd.disable_slot(slot, cycle))
            .await;

        self::scheme::handle_event_trb("DISABLE_SLOT", &event_trb, &command_trb)?;
        //self.event_handler_finished();

        Ok(())
    }

    pub fn slot_state(&self, slot: usize) -> u8 {
        ((self.dev_ctx.contexts[slot].slot.d.read() & SLOT_CONTEXT_STATE_MASK)
            >> SLOT_CONTEXT_STATE_SHIFT) as u8
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

    pub async fn attach_device(&self, port_id: PortId) -> syscall::Result<()> {
        if self.port_states.contains_key(&port_id) {
            debug!("Already contains port {}", port_id);
            return Err(syscall::Error::new(EAGAIN));
        }

        let (data, state, speed, flags) = {
            let port = &self.ports.lock().unwrap()[port_id.root_hub_port_index()];
            (port.read(), port.state(), port.speed(), port.flags())
        };

        info!(
            "XHCI Port {}: {:X}, State {}, Speed {}, Flags {:?}",
            port_id, data, state, speed, flags
        );

        if flags.contains(port::PortFlags::CCS) {
            let slot_ty = match self.supported_protocol(port_id) {
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
                    error!("Failed to enable slot for port {}: {}", port_id, err);
                    return Err(err);
                }
            };

            debug!("Enabled port {}, which the xHC mapped to {}", port_id, slot);

            //TODO: get correct speed for child devices
            let protocol_speed = self
                .lookup_psiv(port_id, speed)
                .expect("Failed to retrieve speed ID");

            let mut input = unsafe { self.alloc_dma_zeroed::<InputContext<N>>()? };

            info!("Attempting to address the device");
            let mut ring = match self
                .address_device(&mut input, port_id, slot_ty, slot, protocol_speed, speed)
                .await
            {
                Ok(device_ring) => device_ring,
                Err(err) => {
                    error!("Failed to address device for port {}: `{}`", port_id, err);
                    return Err(err);
                }
            };

            debug!("Addressed device");

            // TODO: Should the descriptors be cached in PortState, or refetched?

            let mut port_state = PortState {
                slot,
                protocol_speed,
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
            self.port_states.insert(port_id, port_state);
            debug!("Got port states!");

            // Ensure correct packet size is used
            let dev_desc_8_byte = self.fetch_dev_desc_8_byte(port_id, slot).await?;
            {
                let mut port_state = self.port_states.get_mut(&port_id).unwrap();

                let mut input = port_state.input_context.lock().unwrap();

                self.update_max_packet_size(&mut *input, slot, dev_desc_8_byte)
                    .await?;
            }

            debug!("Got the 8 byte dev descriptor: {:X?}", dev_desc_8_byte);

            let dev_desc = self.get_desc(port_id, slot).await?;
            debug!("Got the full device descriptor!");
            self.port_states.get_mut(&port_id).unwrap().dev_desc = Some(dev_desc);

            debug!("Got the port states again!");
            {
                let mut port_state = self.port_states.get_mut(&port_id).unwrap();

                let mut input = port_state.input_context.lock().unwrap();
                debug!("Got the input context!");
                let dev_desc = port_state.dev_desc.as_ref().unwrap();

                self.update_default_control_pipe(&mut *input, slot, dev_desc)
                    .await?;
            }

            debug!("Updated the default control pipe");

            match self.spawn_drivers(port_id) {
                Ok(()) => (),
                Err(err) => {
                    error!("Failed to spawn driver for port {}: `{}`", port_id, err)
                }
            }
        } else {
            warn!("Attempted to attach a device that didnt have CCS=1");
        }

        Ok(())
    }

    pub async fn detach_device(&self, port_id: PortId) -> Result<()> {
        if let Some(children) = self.drivers.remove(&port_id) {
            for mut child in children {
                info!("killing driver process {} for port {}", child.id(), port_id);
                match child.kill() {
                    Ok(()) => {
                        info!("killed driver process {} for port {}", child.id(), port_id);
                        match child.try_wait() {
                            Ok(status_opt) => match status_opt {
                                Some(status) => {
                                    debug!(
                                        "driver process {} for port {} exited with status {}",
                                        child.id(),
                                        port_id,
                                        status
                                    );
                                }
                                None => {
                                    //TODO: kill harder
                                    warn!(
                                        "driver process {} for port {} still running",
                                        child.id(),
                                        port_id
                                    );
                                }
                            },
                            Err(err) => {
                                warn!(
                                    "failed to wait for the driver process {} for port {}: {}",
                                    child.id(),
                                    port_id,
                                    err
                                );
                            }
                        }
                    }
                    Err(err) => {
                        warn!(
                            "failed to kill the driver process {} for port {}: {}",
                            child.id(),
                            port_id,
                            err
                        );
                    }
                }
            }
        }

        if let Some(state) = self.port_states.remove(&port_id) {
            debug!("disabling port slot {} for port {}", state.slot, port_id);
            let result = self.disable_port_slot(state.slot).await;
            debug!(
                "disabled port slot {} for port {} with result: {:?}",
                state.slot, port_id, result
            );

            result
        } else {
            debug!(
                "Attempted to detach from port {}, which wasn't previously attached.",
                port_id
            );
            Ok(())
        }
    }

    pub async fn update_max_packet_size(
        &self,
        input_context: &mut Dma<InputContext<N>>,
        slot_id: u8,
        dev_desc: usb::DeviceDescriptor8Byte,
    ) -> Result<()> {
        let new_max_packet_size = if dev_desc.major_usb_vers() <= 2 {
            // For USB 2.0 and below, packet_size is in bytes
            u32::from(dev_desc.packet_size)
        } else {
            // For later USB versions, packet_size is the shift
            1u32 << dev_desc.packet_size
        };
        let mut b = input_context.device.endpoints[0].b.read();
        b &= 0x0000_FFFF;
        b |= (new_max_packet_size) << 16;
        input_context.device.endpoints[0].b.write(b);

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
            })
            .await;

        self::scheme::handle_event_trb("EVALUATE_CONTEXT", &event_trb, &command_trb)?;
        //self.event_handler_finished();

        Ok(())
    }

    pub async fn update_default_control_pipe(
        &self,
        input_context: &mut Dma<InputContext<N>>,
        slot_id: u8,
        dev_desc: &DevDesc,
    ) -> Result<()> {
        debug!("Updating default control pipe!");
        input_context.add_context.write(1 << 1);
        input_context.drop_context.write(0);

        let new_max_packet_size = if dev_desc.major_version() <= 2 {
            // For USB 2.0 and below, packet_size is in bytes
            u32::from(dev_desc.packet_size)
        } else {
            // For later USB versions, packet_size is the shift
            1u32 << dev_desc.packet_size
        };
        let mut b = input_context.device.endpoints[0].b.read();
        b &= 0x0000_FFFF;
        b |= (new_max_packet_size) << 16;
        input_context.device.endpoints[0].b.write(b);

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.evaluate_context(slot_id, input_context.physical(), false, cycle)
            })
            .await;
        debug!("Completed the command to update the default control pipe");

        self::scheme::handle_event_trb("EVALUATE_CONTEXT", &event_trb, &command_trb)?;
        //self.event_handler_finished();

        Ok(())
    }

    pub async fn address_device(
        &self,
        input_context: &mut Dma<InputContext<N>>,
        port: PortId,
        slot_ty: u8,
        slot: u8,
        protocol_speed: &ProtocolSpeed,
        speed: u8,
    ) -> Result<Ring> {
        // Collect MTT, parent port number, parent slot ID
        let mut mtt = false;
        let mut parent_hub_slot_id = 0u8;
        let mut parent_port_num = 0u8;
        if let Some((parent_port, port_num)) = port.parent() {
            match self.port_states.get(&parent_port) {
                Some(parent_state) => {
                    // parent info must be supplied if:
                    let mut needs_parent_info = false;
                    // 1. the device is low or full speed and connected through a high speed hub
                    //TODO: determine device speed (speed is not accurate as it comes from the port)
                    // 2. the device is superspeed and connected through a higher rank hub
                    //TODO: determine device speed (speed is not accurate as it comes from the port)
                    // For now, this is just set to true to force things to work
                    needs_parent_info = true;
                    if needs_parent_info {
                        parent_hub_slot_id = parent_state.slot;
                        parent_port_num = port_num;
                    }
                    info!(
                        "port {} parent_hub_slot_id {} parent_port_num {}",
                        port, parent_hub_slot_id, parent_port_num
                    );
                }
                None => {
                    warn!("port {} missing parent port {} state", port, parent_port);
                }
            }
        }

        let mut ring = Ring::new::<N>(self.cap.ac64(), 16, true)?;

        {
            input_context.add_context.write(1 << 1 | 1); // Enable the slot (zeroth bit) and the control endpoint (first bit).

            let route_string = port.route_string;
            let context_entries = 1u8;
            let hub = false;

            assert_eq!(route_string & 0x000F_FFFF, route_string);
            input_context.device.slot.a.write(
                route_string
                    | (u32::from(speed) << 20)
                    | (u32::from(mtt) << 25)
                    | (u32::from(hub) << 26)
                    | (u32::from(context_entries) << 27),
            );

            let max_exit_latency = 0u16;
            let root_hub_port_num = port.root_hub_port_num;
            let number_of_ports = 0u8;
            input_context.device.slot.b.write(
                u32::from(max_exit_latency)
                    | (u32::from(root_hub_port_num) << 16)
                    | (u32::from(number_of_ports) << 24),
            );

            // TODO
            let ttt = 0u8;
            let interrupter = 0u8;

            assert_eq!(ttt & 0b11, ttt);
            input_context.device.slot.c.write(
                u32::from(parent_hub_slot_id)
                    | (u32::from(parent_port_num) << 8)
                    | (u32::from(ttt) << 16)
                    | (u32::from(interrupter) << 22),
            );

            let max_error_count = 3u8; // recommended value according to the XHCI spec
            let ep_ty = 4u8; // control endpoint, bidirectional
            let max_packet_size: u32 =
                if protocol_speed.is_lowspeed() || protocol_speed.is_fullspeed() {
                    8
                } else if protocol_speed.is_highspeed() {
                    64
                } else {
                    512
                };
            let host_initiate_disable = false; // only applies to streams
            let max_burst_size = 0u8; // TODO

            assert_eq!(max_error_count & 0b11, max_error_count);
            input_context.device.endpoints[0].b.write(
                (u32::from(max_error_count) << 1)
                    | (u32::from(ep_ty) << 3)
                    | (u32::from(host_initiate_disable) << 7)
                    | (u32::from(max_burst_size) << 8)
                    | (u32::from(max_packet_size) << 16),
            );

            let dequeue_cycle_state = true;
            let tr = ring.register();
            input_context.device.endpoints[0]
                .trh
                .write((tr >> 32) as u32);
            input_context.device.endpoints[0]
                .trl
                .write((tr as u32) | u32::from(dequeue_cycle_state));

            // The default control pipe can always use 8 bytes
            let avg_trb_len = 8u8;
            input_context.device.endpoints[0]
                .c
                .write(u32::from(avg_trb_len));
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
                port,
                event_trb.completion_code()
            );
            //self.event_handler_finished();
            return Err(Error::new(EIO));
        }
        //self.event_handler_finished();

        Ok(ring)
    }

    fn uses_msi_interrupts(&self) -> bool {
        matches!(self.interrupt_method, InterruptMethod::Msi)
    }

    /// Checks whether an IRQ has been received from *this* device, in case of an interrupt. Always
    /// true when using MSI/MSI-X.
    pub fn received_irq(&self) -> bool {
        let mut runtime_regs = self.run.lock().unwrap();

        if self.uses_msi_interrupts() {
            // Since using MSI and MSI-X implies having no IRQ sharing whatsoever, the IP bit
            // doesn't have to be touched.
            trace!(
                "Successfully received MSI/MSI-X interrupt, IP={}, EHB={}",
                runtime_regs.ints[0].iman.readf(1),
                runtime_regs.ints[0].erdp_low.readf(1 << 3)
            );
            true
        } else if runtime_regs.ints[0].iman.readf(1) {
            trace!(
                "Successfully received INTx# interrupt, IP={}, EHB={}",
                runtime_regs.ints[0].iman.readf(1),
                runtime_regs.ints[0].erdp_low.readf(1 << 3)
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
    fn spawn_drivers(&self, port: PortId) -> Result<()> {
        // TODO: There should probably be a way to select alternate interfaces, and not just the
        // first one.
        // TODO: Now that there are some good error crates, I don't think errno.h error codes are
        // suitable here.

        let ps = self.port_states.get(&port).unwrap();
        trace!("Spawning driver on port: {}", port);

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

        trace!("Got config and device descriptors on port {}", port);
        let drivers_usercfg: &DriversConfig = &DRIVERS_CONFIG;

        for ifdesc in config_desc.interface_descs.iter() {
            //TODO: support alternate settings
            // This is difficult because the device driver must know which alternate
            // to use, but if alternates can have different classes, then a different
            // device driver may be required for each alternate. For now, we will use
            // only the default alternate setting (0)
            if ifdesc.alternate_setting != 0 {
                warn!(
                    "ignoring port {} iface {} alternate {} class {}.{} proto {}",
                    port,
                    ifdesc.number,
                    ifdesc.alternate_setting,
                    ifdesc.class,
                    ifdesc.sub_class,
                    ifdesc.protocol
                );
                continue;
            }

            if let Some(driver) = drivers_usercfg.drivers.iter().find(|driver| {
                driver.class == ifdesc.class
                    && driver
                        .subclass()
                        .map(|subclass| subclass == ifdesc.sub_class)
                        .unwrap_or(true)
            }) {
                info!(
                    "Loading subdriver \"{}\" for port {} iface {} alternate {} class {}.{} proto {}",
                    driver.name,
                    port,
                    ifdesc.number,
                    ifdesc.alternate_setting,
                    ifdesc.class,
                    ifdesc.sub_class,
                    ifdesc.protocol,
                );
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
                self.drivers.alter(port, |children_opt| {
                    let mut children = children_opt.unwrap_or_else(|| Vec::new());
                    children.push(process);
                    Some(children)
                });
            } else {
                warn!(
                    "No driver for port {} iface {} alternate {} class {}.{} proto {}",
                    port,
                    ifdesc.number,
                    ifdesc.alternate_setting,
                    ifdesc.class,
                    ifdesc.sub_class,
                    ifdesc.protocol
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
    pub fn supported_protocol(&self, port: PortId) -> Option<&'static SupportedProtoCap> {
        self.supported_protocols_iter().find(|supp_proto| {
            supp_proto
                .compat_port_range()
                .contains(&port.root_hub_port_num)
        })
    }
    pub fn supported_protocol_speeds(
        &self,
        port: PortId,
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
    pub fn lookup_psiv(&self, port: PortId, psiv: u8) -> Option<&'static ProtocolSpeed> {
        self.supported_protocol_speeds(port)
            .find(|speed| speed.psiv() == psiv)
    }
}
pub fn start_irq_reactor<const N: usize>(hci: &Arc<Xhci<N>>, irq_file: Option<File>) {
    let hci_clone = Arc::clone(&hci);

    debug!("About to start IRQ reactor");

    *hci.irq_reactor.lock().unwrap() = Some(thread::spawn(move || {
        debug!("Started IRQ reactor thread");
        IrqReactor::new(hci_clone, irq_file).run()
    }));
}

pub fn start_device_enumerator<const N: usize>(hci: &Arc<Xhci<N>>) {
    let hci_clone = Arc::clone(&hci);

    debug!("About to start Device Enumerator");

    *hci.device_enumerator.lock().unwrap() = Some(thread::spawn(move || {
        debug!("Started Device Enumerator");
        DeviceEnumerator::new(hci_clone).run();
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

use crate::xhci::device_enumerator::{DeviceEnumerationRequest, DeviceEnumerator};
use crate::xhci::port::PortFlags;
use lazy_static::lazy_static;

lazy_static! {
    static ref DRIVERS_CONFIG: DriversConfig = {
        // TODO: Load this at runtime.
        const TOML: &'static [u8] = include_bytes!("../../drivers.toml");

        toml::from_slice::<DriversConfig>(TOML).expect("Failed to parse internally embedded config file")
    };
}
