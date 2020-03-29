use std::convert::TryFrom;
use std::io::prelude::*;
use std::sync::atomic;
use std::{cmp, io, mem, path, str};

use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use smallvec::{smallvec, SmallVec};

use syscall::io::{Dma, Io};
use syscall::scheme::Scheme;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EBADFD, EBADMSG, EEXIST, EINVAL, EIO, EISDIR, ENOENT,
    ENOSYS, ENOTDIR, ENXIO, EOPNOTSUPP, EOVERFLOW, EPERM, EPROTO, ESPIPE, MODE_CHR, MODE_DIR,
    MODE_FILE, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR, O_STAT, O_WRONLY, SEEK_CUR, SEEK_END,
    SEEK_SET,
};

use super::{port, usb};
use super::{EndpointState, Xhci};

use super::context::{
    InputContext, SlotState, StreamContext, StreamContextArray, StreamContextType,
    ENDPOINT_CONTEXT_STATUS_MASK,
};
use super::doorbell::Doorbell;
use super::extended::ProtocolSpeed;
use super::irq_reactor::RingId;
use super::operational::OperationalRegs;
use super::ring::Ring;
use super::runtime::RuntimeRegs;
use super::trb::{TransferKind, Trb, TrbCompletionCode, TrbType};
use super::usb::endpoint::{EndpointTy, ENDP_ATTR_TY_MASK};

use crate::driver_interface::*;

pub enum ControlFlow {
    Continue,
    Break,
}

#[derive(Clone, Copy, Debug)]
pub enum EndpIfState {
    Init,
    WaitingForDataPipe {
        direction: XhciEndpCtlDirection,
        bytes_transferred: u32,
        bytes_to_transfer: u32,
    },
    WaitingForStatus,
    WaitingForTransferResult(PortTransferStatus),
}

/// Subdirs of an endpoint
pub enum EndpointHandleTy {
    /// portX/endpoints/Y/data. Allows clients to read and write data associated with ctl requests.
    Data,

    /// portX/endpoints/Y/status
    Ctl,

    /// portX/endpoints/Y/
    Root(usize, Vec<u8>), // offset, content
}

#[derive(Clone, Copy)]
pub enum PortTransferState {
    /// Ready to read or write to do another transfer
    Ready,

    /// Transfer has completed, and the status has to be read.
    WaitingForStatusReq(PortTransferStatus),
}

pub enum PortReqState {
    Init,
    WaitingForDeviceBytes(Dma<[u8]>, usb::Setup), // buffer, setup params
    WaitingForHostBytes(Dma<[u8]>, usb::Setup),   // buffer, setup params
    TmpSetup(usb::Setup),
    Tmp,
}

pub enum Handle {
    TopLevel(usize, Vec<u8>),              // offset, contents (ports)
    Port(usize, usize, Vec<u8>),           // port, offset, contents
    PortDesc(usize, usize, Vec<u8>),       // port, offset, contents
    PortState(usize, usize),               // port, offset
    PortReq(usize, PortReqState),          // port, state
    Endpoints(usize, usize, Vec<u8>),      // port, offset, contents
    Endpoint(usize, u8, EndpointHandleTy), // port, endpoint, offset, state
    ConfigureEndpoints(usize),             // port
}

// TODO: Even though the driver interface descriptors are originally intended for JSON, they should suffice... for
// now.

impl From<usb::EndpointDescriptor> for EndpDesc {
    fn from(d: usb::EndpointDescriptor) -> Self {
        Self {
            kind: d.kind,
            address: d.address,
            attributes: d.attributes,
            interval: d.interval,
            max_packet_size: d.max_packet_size,
            ssc: None,
            sspc: None,
        }
    }
}

impl From<usb::HidDescriptor> for HidDesc {
    fn from(d: usb::HidDescriptor) -> Self {
        Self {
            kind: d.kind,
            hid_spec_release: d.hid_spec_release,
            country: d.country_code,
            desc_count: d.num_descriptors,
            desc_ty: d.report_desc_ty,
            desc_len: d.report_desc_len,
            optional_desc_ty: d.optional_desc_ty,
            optional_desc_len: d.optional_desc_len,
        }
    }
}

impl From<usb::SuperSpeedCompanionDescriptor> for SuperSpeedCmp {
    fn from(d: usb::SuperSpeedCompanionDescriptor) -> Self {
        Self {
            kind: d.kind,
            attributes: d.attributes,
            bytes_per_interval: d.bytes_per_interval,
            max_burst: d.max_burst,
        }
    }
}
impl From<usb::SuperSpeedPlusIsochCmpDescriptor> for SuperSpeedPlusIsochCmp {
    fn from(r: usb::SuperSpeedPlusIsochCmpDescriptor) -> Self {
        Self {
            kind: r.kind,
            bytes_per_interval: r.bytes_per_interval,
        }
    }
}

/// Any descriptor that can be stored in the config desc "data" area.
#[derive(Debug)]
pub enum AnyDescriptor {
    // These are the ones that I have found, but there are more.
    Device(usb::DeviceDescriptor),
    Config(usb::ConfigDescriptor),
    Interface(usb::InterfaceDescriptor),
    Endpoint(usb::EndpointDescriptor),
    Hid(usb::HidDescriptor),
    SuperSpeedCompanion(usb::SuperSpeedCompanionDescriptor),
    SuperSpeedPlusCompanion(usb::SuperSpeedPlusIsochCmpDescriptor),
}

impl AnyDescriptor {
    fn parse(bytes: &[u8]) -> Option<(Self, usize)> {
        if bytes.len() < 2 {
            return None;
        }

        let len = bytes[0];
        let kind = bytes[1];

        if bytes.len() < len.into() {
            return None;
        }

        Some((
            match kind {
                1 => Self::Device(*plain::from_bytes(bytes).ok()?),
                2 => Self::Config(*plain::from_bytes(bytes).ok()?),
                4 => Self::Interface(*plain::from_bytes(bytes).ok()?),
                5 => Self::Endpoint(*plain::from_bytes(bytes).ok()?),
                33 => Self::Hid(*plain::from_bytes(bytes).ok()?),
                48 => Self::SuperSpeedCompanion(*plain::from_bytes(bytes).ok()?),
                49 => Self::SuperSpeedPlusCompanion(*plain::from_bytes(bytes).ok()?),
                _ => {
                    //panic!("Descriptor unknown {}: bytes {:#0x?}", kind, bytes);
                    return None;
                }
            },
            len.into(),
        ))
    }
}

impl Xhci {
    async fn new_if_desc(
        &self,
        port_id: usize,
        slot: u8,
        desc: usb::InterfaceDescriptor,
        endps: impl IntoIterator<Item = EndpDesc>,
        hid_descs: impl IntoIterator<Item = HidDesc>,
    ) -> Result<IfDesc> {
        Ok(IfDesc {
            alternate_setting: desc.alternate_setting,
            class: desc.class,
            interface_str: if desc.interface_str > 0 {
                Some(self.fetch_string_desc(port_id, slot, desc.interface_str).await?)
            } else {
                None
            },
            kind: desc.kind,
            number: desc.number,
            protocol: desc.protocol,
            sub_class: desc.sub_class,
            endpoints: endps.into_iter().collect(),
            hid_descs: hid_descs.into_iter().collect(),
        })
    }
    /// Pushes a command TRB to the command ring, rings the doorbell, and then awaits its Command
    /// Completion Event.
    ///
    /// # Locking
    /// This function will lock `Xhci::cmd` and `Xhci::dbs`.
    pub async fn execute_command<F: FnOnce(&mut Trb, bool)>(
        &self,
        f: F,
    ) -> (Trb, Trb) {
        let next_event = {
            let mut command_ring = self.cmd.lock().unwrap();
            let (cmd_index, cycle) = (command_ring.next_index(), command_ring.cycle);

            {
                let command_trb = &mut command_ring.trbs[cmd_index];
                f(command_trb, cycle);
            }

            // get the future here before awaiting, to destroy the lock before deadlock
            let command_trb = &command_ring.trbs[cmd_index];
            self.next_command_completion_event_trb(&*command_ring, command_trb)
        };

        println!("Ringing doorbell");
        self.dbs.lock().unwrap()[0].write(0);
        println!("Doorbell rung");

        let trbs = next_event.await;
        let event_trb = trbs.event_trb;
        let command_trb = trbs.src_trb.expect("Command completion event TRBs shall always have a valid pointer to a valid source command TRB");

        assert_eq!(event_trb.trb_type(), TrbType::CommandCompletion as u8, "The IRQ reactor (or the xHC) gave an invalid event TRB");

        (event_trb, command_trb)
    }
    pub async fn execute_control_transfer<D>(
        &self,
        port_num: usize,
        setup: usb::Setup,
        tk: TransferKind,
        name: &str,
        mut d: D,
    ) -> Result<Trb>
    where
        D: FnMut(&mut Trb, bool) -> ControlFlow,
    {
        let mut port_state = self.port_state_mut(port_num)?;
        let slot = port_state.slot;

        let future = {
            let mut endpoint_state = port_state
                .endpoint_states
                .get_mut(&0).ok_or(Error::new(EIO))?;

            let ring = endpoint_state
                .ring()
                .ok_or(Error::new(EIO))?;

            let (cmd, cycle) = ring.next();
            cmd.setup(setup, tk, cycle);

            if tk != TransferKind::NoData {
                loop {
                    let (trb, cycle) = ring.next();
                    match d(trb, cycle) {
                        ControlFlow::Break => break,
                        ControlFlow::Continue => continue,
                    }
                }
            }

            let last_index = ring.next_index();
            let (cmd, cycle) = (&mut ring.trbs[last_index], ring.cycle);

            let interrupter = 0;
            let ioc = true;
            let ch = false;
            let ent = false;

            cmd.status(interrupter, tk == TransferKind::In, ioc, ch, ent, cycle);
            self.next_transfer_event_trb(RingId::default_control_pipe(port_num as u8), ring, &ring.trbs[last_index])
        };

        self.dbs.lock().unwrap()[usize::from(slot)].write(Self::def_control_endp_doorbell());

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let status_trb = trbs.src_trb.unwrap();

        handle_transfer_event_trb("CONTROL_TRANSFER", &event_trb, &status_trb)?;

        self.event_handler_finished();

        Ok(event_trb)
    }
    /// NOTE: There has to be AT LEAST one successful invocation of `d`, that actually updates the
    /// TRB (it could be a NO-OP in the worst case).
    /// The function is also required to set the Interrupt on Completion flag, or this function
    /// will never complete.
    pub async fn execute_transfer<D>(
        &self,
        port_num: usize,
        endp_num: u8,
        stream_id: u16,
        name: &str,
        mut d: D,
    ) -> Result<Trb>
    where
        D: FnMut(&mut Trb, bool) -> ControlFlow,
    {
        let mut port_state = self.port_state_mut(port_num)?;

        let (cfg_idx, if_idx) = match (port_state.cfg_idx, port_state.if_idx) {
            (Some(c), Some(i)) => (c, i),
            _ => return Err(Error::new(EIO)),
        };


        let slot = port_state.slot;

        let endp_state = port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADF))?;

        let (has_streams, ring) = match endp_state {
            EndpointState {
                transfer: super::RingOrStreams::Ring(ref mut ring),
                ..
            } => (false, ring),
            EndpointState {
                transfer: super::RingOrStreams::Streams(stream_ctx_array),
                ..
            } => (true, stream_ctx_array
                .rings
                .get_mut(&1)
                .ok_or(Error::new(EBADF))?),
        };

        let future = loop {
            let last_index = ring.next_index();
            let (trb, cycle) = (&mut ring.trbs[last_index], ring.cycle);

            match d(trb, cycle) {
                ControlFlow::Break => {
                    break self.next_transfer_event_trb(super::irq_reactor::RingId { port: port_num as u8, endpoint_num: endp_num, stream_id }, ring, &ring.trbs[last_index]);
                }
                ControlFlow::Continue => continue,
            }
        };

        let endp_desc = port_state.dev_desc.as_ref().unwrap().config_descs[usize::from(cfg_idx)].interface_descs[usize::from(if_idx)].endpoints.get(usize::from(endp_num)).ok_or(Error::new(EBADFD))?;

        self.dbs.lock().unwrap()[usize::from(slot)].write(Self::endp_doorbell(
            endp_num,
            endp_desc,
            if has_streams { stream_id } else { 0 },
        ));

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let transfer_trb = trbs.src_trb.unwrap();

        handle_transfer_event_trb("EXECUTE_TRANSFER", &event_trb, &transfer_trb)?;

        // FIXME: EDTLA if event data was set
        if event_trb.completion_code() != TrbCompletionCode::ShortPacket as u8
            && event_trb.transfer_length() != 0
        {
            println!(
                "Event trb didn't yield a short packet, but some bytes were not transferred"
            );
            return Err(Error::new(EIO));
        }

        // TODO: Handle event data
        println!("EVENT DATA: {:?}", event_trb.event_data());

        Ok(event_trb)
    }
    async fn device_req_no_data(&self, port: usize, req: usb::Setup) -> Result<()> {
        self.execute_control_transfer(
            port,
            req,
            TransferKind::NoData,
            "DEVICE_REQ_NO_DATA",
            |_, _| ControlFlow::Break,
        ).await?;
        Ok(())
    }
    async fn set_configuration(&self, port: usize, config: u8) -> Result<()> {
        self.device_req_no_data(port, usb::Setup::set_configuration(config)).await
    }
    async fn set_interface(
        &self,
        port: usize,
        interface_num: u8,
        alternate_setting: u8,
    ) -> Result<()> {
        self.device_req_no_data(
            port,
            usb::Setup::set_interface(interface_num, alternate_setting),
        ).await
    }

    async fn reset_endpoint(&self, port_num: usize, endp_num: u8, tsp: bool) -> Result<()> {
        let port_state = self.port_states.get(&port_num).ok_or(Error::new(EBADFD))?;

        let (cfg_idx, if_idx) = match (port_state.cfg_idx, port_state.if_idx) {
            (Some(c), Some(i)) => (c, i),
            _ => return Err(Error::new(EIO)),
        };

        let endp_desc = port_state.dev_desc.as_ref().unwrap().config_descs[usize::from(cfg_idx)].interface_descs[usize::from(if_idx)].endpoints.get(usize::from(endp_num)).ok_or(Error::new(EBADFD))?;
        let endp_num_xhc = Self::endp_num_to_dci(endp_num, endp_desc);

        let slot = self
            .port_states
            .get(&port_num)
            .ok_or(Error::new(EBADF))?
            .slot;

        let (event_trb, command_trb) = self.execute_command(|trb, cycle| {
            trb.reset_endpoint(slot, endp_num_xhc, tsp, cycle);
        }).await;
        self.event_handler_finished();

        handle_event_trb("RESET_ENDPOINT", &event_trb, &command_trb)
    }

    fn endp_ctx_interval(speed_id: &ProtocolSpeed, endp_desc: &EndpDesc) -> u8 {
        /// Logarithmic (base 2) 125 µs periods per millisecond.
        const MILLISEC_PERIODS: u8 = 3;

        // TODO: Also check the Speed ID for superspeed(plus).
        if (speed_id.is_lowspeed() || speed_id.is_fullspeed()) && endp_desc.is_interrupt() {
            // The interval field has values 1-255, ranging from 1 ms to 255 ms.
            // TODO: This is correct, right?
            let last_power_of_two = 8 - endp_desc.interval.leading_zeros() as u8;
            last_power_of_two - 1 + MILLISEC_PERIODS
        } else if speed_id.is_fullspeed() && endp_desc.is_isoch() {
            // bInterval has values 1-16, ranging from 1 ms to 32,768 ms.
            endp_desc.interval - 1 + MILLISEC_PERIODS
        } else if (speed_id.is_fullspeed()
            || endp_desc.is_superspeed()
            || endp_desc.is_superspeedplus())
            && (endp_desc.is_interrupt() || endp_desc.is_isoch())
        {
            // bInterval has values 1-16, but ranging from 125 µs to 4096 ms.
            endp_desc.interval - 1
        } else {
            // This includes superspeed(plus) control and bulk endpoints in particular.
            0
        }
    }
    fn endp_ctx_max_burst(
        speed_id: &ProtocolSpeed,
        dev_desc: &DevDesc,
        endp_desc: &EndpDesc,
    ) -> u8 {
        if speed_id.is_highspeed() && (endp_desc.is_interrupt() || endp_desc.is_isoch()) {
            assert_eq!(dev_desc.major_version(), 2);
            ((endp_desc.max_packet_size & 0x0C00) >> 11) as u8
        } else if endp_desc.is_superspeed() {
            endp_desc.max_burst()
        } else {
            0
        }
    }
    fn endp_ctx_max_packet_size(endp_desc: &EndpDesc) -> u16 {
        // TODO: Control endpoint? Encoding?
        endp_desc.max_packet_size & 0x07FF
    }
    fn endp_ctx_max_esit_payload(
        speed_id: &ProtocolSpeed,
        dev_desc: &DevDesc,
        endp_desc: &EndpDesc,
        max_packet_size: u16,
        max_burst_size: u8,
    ) -> u32 {
        const KIB: u32 = 1024;

        if dev_desc.major_version() == 2 && endp_desc.is_periodic() {
            u32::from(max_packet_size) * (u32::from(max_burst_size) + 1)
        } else if !endp_desc.has_ssp_companion() {
            u32::from(endp_desc.ssc.as_ref().unwrap().bytes_per_interval)
        } else if endp_desc.has_ssp_companion() {
            endp_desc.sspc.as_ref().unwrap().bytes_per_interval
        } else if speed_id.is_fullspeed() && endp_desc.is_interrupt() {
            64
        } else if speed_id.is_fullspeed() && endp_desc.is_isoch() {
            1 * KIB
        } else if (speed_id.is_highspeed() && (endp_desc.is_interrupt() || endp_desc.is_isoch()))
            || endp_desc.is_superspeed() && endp_desc.is_interrupt()
        {
            3 * KIB
        } else if endp_desc.is_superspeed() && endp_desc.is_isoch() {
            48 * KIB
        } else {
            // TODO: Is "maximum allowed" ESIT payload, the same as "maximum" ESIT payload.
            0
        }
    }

    fn port_state(&self, port: usize) -> Result<chashmap::ReadGuard<'_, usize, super::PortState>> {
        self.port_states.get(&port).ok_or(Error::new(EBADF))
    }
    fn port_state_mut(&self, port: usize) -> Result<chashmap::WriteGuard<'_, usize, super::PortState>> {
        self.port_states.get_mut(&port).ok_or(Error::new(EBADF))
    }
    async fn configure_endpoints(&self, port: usize, json_buf: &[u8]) -> Result<()> {
        let mut req: ConfigureEndpointsReq =
            serde_json::from_slice(json_buf).or(Err(Error::new(EBADMSG)))?;

        if (!self.cap.cic() || !self.op.lock().unwrap().cie())
            && (req.config_desc != 0 || req.interface_desc != None || req.alternate_setting != None)
        {
            //return Err(Error::new(EOPNOTSUPP));
            req.config_desc = 0;
            req.alternate_setting = None;
            req.interface_desc = None;
        }
        if req.interface_desc.is_some() != req.alternate_setting.is_some() {
            return Err(Error::new(EBADMSG));
        }

        let (endp_desc_count, new_context_entries, configuration_value) = {
            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            let config_desc = port_state.dev_desc.as_ref().unwrap().config_descs.get(usize::from(req.config_desc)).ok_or(Error::new(EBADFD))?;

            let endpoints = &config_desc.interface_descs.get(usize::from(req.interface_desc.unwrap_or(0))).ok_or(Error::new(EBADFD))?.endpoints;

            if endpoints.len() >= 31 {
                return Err(Error::new(EIO));
            }

            (
                endpoints.len(),
                (match endpoints.last() {
                    Some(l) => Self::endp_num_to_dci(endpoints.len() as u8, l),
                    None => 1,
                }) + 1,
                config_desc.configuration_value,
            )
        };
        let lec = self.cap.lec();
        let log_max_psa_size = self.cap.max_psa_size();

        let port_speed_id = self.ports.lock().unwrap()[port].speed();
        let speed_id: &ProtocolSpeed = self
            .lookup_psiv(port as u8, port_speed_id)
            .ok_or(Error::new(EIO))?;

        {
            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            let mut input_context = port_state.input_context.lock().unwrap();

            // Configure the slot context as well, which holds the last index of the endp descs.
            input_context.add_context.write(1);
            input_context.drop_context.write(0);

            const CONTEXT_ENTRIES_MASK: u32 = 0xF800_0000;
            const CONTEXT_ENTRIES_SHIFT: u8 = 27;

            let current_slot_a = input_context.device.slot.a.read();

            input_context.device.slot.a.write(
                (current_slot_a & !CONTEXT_ENTRIES_MASK)
                    | ((u32::from(new_context_entries) << CONTEXT_ENTRIES_SHIFT)
                        & CONTEXT_ENTRIES_MASK),
            );
            input_context.control.write(
                (u32::from(req.alternate_setting.unwrap_or(0)) << 16)
                    | (u32::from(req.interface_desc.unwrap_or(0)) << 8)
                    | u32::from(req.config_desc),
            );
        }

        for endp_idx in 0..endp_desc_count as u8 {
            let endp_num = endp_idx + 1;

            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            let dev_desc = port_state.dev_desc.as_ref().unwrap();
            let endpoints = &dev_desc.config_descs.get(usize::from(req.config_desc)).ok_or(Error::new(EBADFD))?.interface_descs.get(usize::from(req.interface_desc.unwrap_or(0))).ok_or(Error::new(EBADFD))?.endpoints;
            let endp_desc = endpoints.get(endp_idx as usize).ok_or(Error::new(EIO))?;

            let endp_num_xhc = Self::endp_num_to_dci(endp_num, endp_desc);

            let usb_log_max_streams = endp_desc.log_max_streams();

            // TODO: Secondary streams.
            let primary_streams = if let Some(log_max_streams) = usb_log_max_streams {
                // TODO: Can streams-capable be configured to not use streams?
                if log_max_psa_size != 0 {
                    cmp::min(u8::from(log_max_streams), log_max_psa_size + 1) - 1
                } else {
                    0
                }
            } else {
                0
            };
            let linear_stream_array = if primary_streams != 0 { true } else { false };

            // TODO: Interval related fields
            // TODO: Max ESIT payload size.

            let mult = endp_desc.isoch_mult(lec);

            let max_packet_size = Self::endp_ctx_max_packet_size(endp_desc);
            let max_burst_size = Self::endp_ctx_max_burst(speed_id, dev_desc, endp_desc);

            let max_esit_payload = Self::endp_ctx_max_esit_payload(
                speed_id,
                dev_desc,
                endp_desc,
                max_packet_size,
                max_burst_size,
            );
            let max_esit_payload_lo = max_esit_payload as u16;
            let max_esit_payload_hi = ((max_esit_payload & 0x00FF_0000) >> 16) as u8;

            let interval = Self::endp_ctx_interval(speed_id, endp_desc);

            let max_error_count = 3;
            let ep_ty = endp_desc.xhci_ep_type()?;
            let host_initiate_disable = false;

            // TODO: Maybe this value is out of scope for xhcid, because the actual usb device
            // driver probably knows better. The spec says that the initial value should be 8 bytes
            // for control, 1KiB for interrupt and 3KiB for bulk and isoch.
            let avg_trb_len: u16 = match endp_desc.ty() {
                EndpointTy::Ctrl => return Err(Error::new(EIO)), // only endpoint zero is of type control, and is configured separately with the address device command.
                EndpointTy::Bulk | EndpointTy::Isoch => 3072,    // 3 KiB
                EndpointTy::Interrupt => 1024,                   // 1 KiB
            };

            assert_eq!(ep_ty & 0x7, ep_ty);
            assert_eq!(mult & 0x3, mult);
            assert_eq!(max_error_count & 0x3, max_error_count);
            assert_ne!(ep_ty, 0); // 0 means invalid.

            let mut port_state = self.port_states.get_mut(&port).ok_or(Error::new(EBADFD))?;

            let ring_ptr = if usb_log_max_streams.is_some() {
                let mut array = StreamContextArray::new(1 << (primary_streams + 1))?;

                // TODO: Use as many stream rings as needed.
                array.add_ring(1, true)?;
                let array_ptr = array.register();

                assert_eq!(
                    array_ptr & 0xFFFF_FFFF_FFFF_FF81,
                    array_ptr,
                    "stream ctx ptr not aligned to 16 bytes"
                );
                port_state.endpoint_states.insert(
                    endp_num,
                    EndpointState {
                        transfer: super::RingOrStreams::Streams(array),
                        driver_if_state: EndpIfState::Init,
                    },
                );

                array_ptr
            } else {
                let ring = Ring::new(16, true)?;
                let ring_ptr = ring.register();

                assert_eq!(
                    ring_ptr & 0xFFFF_FFFF_FFFF_FF81,
                    ring_ptr,
                    "ring pointer not aligned to 16 bytes"
                );
                port_state.endpoint_states.insert(
                    endp_num,
                    EndpointState {
                        transfer: super::RingOrStreams::Ring(ring),
                        driver_if_state: EndpIfState::Init,
                    },
                );
                ring_ptr
            };
            assert_eq!(primary_streams & 0x1F, primary_streams);

            let port_state = self.port_states.get_mut(&port).ok_or(Error::new(EBADFD))?;
            let mut input_context = port_state.input_context.lock().unwrap();
            input_context.add_context.writef(1 << endp_num_xhc, true);

            let endp_ctx = input_context.device.endpoints.get_mut(endp_num_xhc as usize - 1).ok_or(Error::new(EIO))?;

            endp_ctx.a.write(
                u32::from(mult) << 8
                    | u32::from(primary_streams) << 10
                    | u32::from(linear_stream_array) << 15
                    | u32::from(interval) << 16
                    | u32::from(max_esit_payload_hi) << 24,
            );
            endp_ctx.b.write(
                max_error_count << 1
                    | u32::from(ep_ty) << 3
                    | u32::from(host_initiate_disable) << 7
                    | u32::from(max_burst_size) << 8
                    | u32::from(max_packet_size) << 16,
            );

            endp_ctx.trl.write(ring_ptr as u32);
            endp_ctx.trh.write((ring_ptr >> 32) as u32);

            endp_ctx
                .c
                .write(u32::from(avg_trb_len) | (u32::from(max_esit_payload_lo) << 16));
        }

        let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
        let slot = port_state.slot;
        let input_context_physical = port_state.input_context.lock().unwrap().physical();

        let (event_trb, command_trb) = self.execute_command(|trb, cycle| {
            trb.configure_endpoint(slot, input_context_physical, cycle)
        }).await;
        self.event_handler_finished();

        handle_event_trb("CONFIGURE_ENDPOINT", &event_trb, &command_trb)?;

        // Tell the device about this configuration.
        self.set_configuration(port, configuration_value).await?;

        if let (Some(interface_num), Some(alternate_setting)) =
            (req.interface_desc, req.alternate_setting)
        {
            self.set_interface(port, interface_num, alternate_setting).await?;
        }

        Ok(())
    }
    async fn transfer_read(
        &self,
        port_num: usize,
        endp_idx: u8,
        buf: &mut [u8],
    ) -> Result<(u8, u32)> {
        if buf.is_empty() {
            return Err(Error::new(EINVAL));
        }
        let dma_buffer = unsafe { Dma::<[u8]>::zeroed_unsized(buf.len())? };

        let (completion_code, bytes_transferred, dma_buffer) = self.transfer(
            port_num,
            endp_idx,
            Some(dma_buffer),
            PortReqDirection::DeviceToHost,
        ).await?;

        buf.copy_from_slice(&*dma_buffer.as_ref().unwrap());
        Ok((completion_code, bytes_transferred))
    }
    async fn transfer_write(&self, port_num: usize, endp_idx: u8, sbuf: &[u8]) -> Result<(u8, u32)> {
        if sbuf.is_empty() {
            return Err(Error::new(EINVAL));
        }
        let mut dma_buffer = unsafe { Dma::<[u8]>::zeroed_unsized(sbuf.len()) }?;
        dma_buffer.copy_from_slice(sbuf);

        let (completion_code, bytes_transferred, _) = self.transfer(
            port_num,
            endp_idx,
            Some(dma_buffer),
            PortReqDirection::HostToDevice,
        ).await?;
        Ok((completion_code, bytes_transferred))
    }
    pub const fn def_control_endp_doorbell() -> u32 {
        1
    }
    // TODO: Wrap DCIs and driver-level endp_num into distinct types, due to the high chance of
    // mixing the two up.
    fn endp_num_to_dci(endp_num: u8, desc: &EndpDesc) -> u8 {
        if endp_num == 0 {
            unreachable!("EndpDesc cannot be obtained from the default control endpoint")
        }

        if desc.is_control() || desc.direction() == EndpDirection::In {
            endp_num * 2 + 1
        } else if desc.direction() == EndpDirection::Out {
            endp_num * 2
        } else {
            unreachable!()
        }
    }
    fn endp_doorbell(endp_num: u8, desc: &EndpDesc, stream_id: u16) -> u32 {
        let db_target = Self::endp_num_to_dci(endp_num, desc);
        let db_task_id: u16 = stream_id;

        (u32::from(db_task_id) << 16) | u32::from(db_target)
    }
    // TODO: Rename DeviceReqData to something more general.
    async fn transfer(
        &self,
        port_num: usize,
        endp_idx: u8,
        dma_buf: Option<Dma<[u8]>>,
        direction: PortReqDirection,
    ) -> Result<(u8, u32, Option<Dma<[u8]>>)> {
        // TODO: Check that only readable enpoints are read, etc.
        let endp_num = endp_idx + 1;

        let port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADFD))?;

        let endp_desc: &EndpDesc = port_state
            .dev_desc
            .as_ref().unwrap()
            .config_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .interface_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .endpoints
            .get(endp_idx as usize)
            .ok_or(Error::new(EBADFD))?;

        let direction = endp_desc.direction();

        if endp_desc.is_isoch() {
            return Err(Error::new(ENOSYS));
        }

        if EndpDirection::from(direction) != endp_desc.direction() {
            return Err(Error::new(EBADF));
        }

        let max_packet_size = endp_desc.max_packet_size;
        let max_transfer_size = 65536u32;

        let (buffer, idt, estimated_td_size) = {
            let (buffer, idt) =
                if dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0) <= 8 && max_packet_size >= 8 && direction != EndpDirection::In {
                    dma_buf.as_ref().map(|sbuf| {
                        let mut bytes = [0u8; 8];
                        bytes[..sbuf.len()].copy_from_slice(&sbuf);
                        (u64::from_le_bytes(bytes), true)
                    })
                    .unwrap_or((0, false))
                } else {
                    (
                        dma_buf.as_ref().map(|dma| dma.physical()).unwrap_or(0) as u64,
                        false,
                    )
                };
            let estimated_td_size = cmp::min(
                u8::try_from(
                    div_round_up(dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0), max_transfer_size as usize) * mem::size_of::<Trb>(),
                )
                .ok()
                .unwrap_or(0x1F),
                0x1F,
            ); // one trb per td
            (buffer, idt, estimated_td_size)
        };

        let stream_id = 1u16;

        let mut bytes_left = dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0);

        let event = self.execute_transfer(
            port_num,
            endp_num,
            stream_id,
            "CUSTOM_TRANSFER",
            |trb, cycle| {
                let len = cmp::min(bytes_left, max_transfer_size as usize) as u32;

                // set the interrupt on completion (IOC) flag for the last trb.
                let ioc = bytes_left <= max_transfer_size as usize;
                let chain = !ioc;

                trb.normal(
                    buffer,
                    len,
                    cycle,
                    estimated_td_size,
                    0,
                    false,
                    true,
                    chain,
                    ioc,
                    idt,
                    false,
                );

                bytes_left -= len as usize;

                if bytes_left != 0 {
                    ControlFlow::Continue
                } else {
                    ControlFlow::Break
                }
            },
        ).await?;
        self.event_handler_finished();

        let bytes_transferred = dma_buf.as_ref().map(|buf| buf.len() as u32 - event.transfer_length()).unwrap_or(0);

        Ok((event.completion_code(), bytes_transferred, dma_buf))
    }
    pub async fn get_desc(
        &self,
        port_id: usize,
        slot: u8,
    ) -> Result<DevDesc> {
        println!("Checkpoint 1");
        let ports = self.ports.lock().unwrap();
        let port = ports.get(port_id).ok_or(Error::new(ENOENT))?;
        if !port.flags().contains(port::PortFlags::PORT_CCS) {
            return Err(Error::new(ENOENT));
        }

        println!("Checkpoint 2");
        let raw_dd = self.fetch_dev_desc(port_id, slot).await?;
        println!("Checkpoint 3");

        let (manufacturer_str, product_str, serial_str) = (
            if raw_dd.manufacturer_str > 0 {
                println!("Checkpoint 4a");
                Some(self.fetch_string_desc(port_id, slot, raw_dd.manufacturer_str).await?)
            } else {
                None
            },
            if raw_dd.product_str > 0 {
                println!("Checkpoint 4b");
                Some(self.fetch_string_desc(port_id, slot, raw_dd.product_str).await?)
            } else {
                None
            },
            if raw_dd.serial_str > 0 {
                println!("Checkpoint 4c");
                Some(self.fetch_string_desc(port_id, slot, raw_dd.serial_str).await?)
            } else {
                None
            },
        );

        println!("Checkpoint 5");
        let (bos_desc, bos_data) = self.fetch_bos_desc(port_id, slot).await?;
        println!("Checkpoint 6");

        let supports_superspeed =
            usb::bos_capability_descs(bos_desc, &bos_data).any(|desc| desc.is_superspeed());
        let supports_superspeedplus =
            usb::bos_capability_descs(bos_desc, &bos_data).any(|desc| desc.is_superspeedplus());

        let mut config_descs = SmallVec::new();

        for index in 0..raw_dd.configurations {
            println!("Checkpoint 7: {}", index);
            let (desc, data) = self.fetch_config_desc(port_id, slot, index).await?;
            println!("Checkpoint 8: {}", index);

            let extra_length = desc.total_length as usize - mem::size_of_val(&desc);
            let data = &data[..extra_length];

            let mut i = 0;
            let mut descriptors = Vec::new();

            while let Some((descriptor, len)) = AnyDescriptor::parse(&data[i..]) {
                descriptors.push(descriptor);
                i += len;
            }

            let mut interface_descs = SmallVec::new();
            let mut iter = descriptors.into_iter();

            while let Some(item) = iter.next() {
                if let AnyDescriptor::Interface(idesc) = item {
                    let mut endpoints = SmallVec::<[EndpDesc; 4]>::new();
                    let mut hid_descs = SmallVec::<[HidDesc; 1]>::new();

                    for _ in 0..idesc.endpoints {
                        let next = match iter.next() {
                            Some(AnyDescriptor::Endpoint(n)) => n,
                            Some(AnyDescriptor::Hid(h)) if idesc.class == 3 => {
                                hid_descs.push(h.into());
                                break;
                            }
                            _ => break,
                        };
                        let mut endp = EndpDesc::from(next);

                        if supports_superspeed {
                            let next = match iter.next() {
                                Some(AnyDescriptor::SuperSpeedCompanion(n)) => n,
                                _ => break,
                            };
                            endp.ssc = Some(SuperSpeedCmp::from(next));

                            if endp.has_ssp_companion() && supports_superspeedplus {
                                let next = match iter.next() {
                                    Some(AnyDescriptor::SuperSpeedPlusCompanion(n)) => n,
                                    _ => break,
                                };
                                endp.sspc = Some(SuperSpeedPlusIsochCmp::from(next));
                            }
                        }
                        endpoints.push(endp);
                    }

                    interface_descs.push(self.new_if_desc(port_id, slot, idesc, endpoints, hid_descs).await?);
                } else {
                    // TODO
                    break;
                }
            }

            config_descs.push(ConfDesc {
                kind: desc.kind,
                configuration: if desc.configuration_str > 0 {
                    Some(self.fetch_string_desc(port_id, slot, desc.configuration_str).await?)
                } else {
                    None
                },
                configuration_value: desc.configuration_value,
                attributes: desc.attributes,
                max_power: desc.max_power,
                interface_descs,
            });
        };

        Ok(DevDesc {
            kind: raw_dd.kind,
            usb: raw_dd.usb,
            class: raw_dd.class,
            sub_class: raw_dd.sub_class,
            protocol: raw_dd.protocol,
            packet_size: raw_dd.packet_size,
            vendor: raw_dd.vendor,
            product: raw_dd.product,
            release: raw_dd.release,
            manufacturer_str,
            product_str,
            serial_str,
            config_descs,
        })
    }
    fn port_desc_json(&self, port_id: usize) -> Result<Vec<u8>> {
        let dev_desc = &self
            .port_states
            .get(&port_id)
            .ok_or(Error::new(ENOENT))?
            .dev_desc;
        serde_json::to_vec(dev_desc).or(Err(Error::new(EIO)))
    }
    fn write_dyn_string(string: &[u8], buf: &mut [u8], offset: &mut usize) -> usize {
        let max_bytes_to_read = cmp::min(string.len(), buf.len());
        let bytes_to_read = cmp::max(*offset, max_bytes_to_read) - *offset;
        buf[..bytes_to_read].copy_from_slice(&string[..bytes_to_read]);

        *offset += bytes_to_read;

        bytes_to_read
    }
    async fn port_req_transfer(
        &self,
        port_num: usize,
        data_buffer: Option<&mut Dma<[u8]>>,
        setup: usb::Setup,
        transfer_kind: TransferKind,
    ) -> Result<()> {
        self.execute_control_transfer(
            port_num,
            setup,
            transfer_kind,
            "CUSTOM_DEVICE_REQ",
            |trb, cycle| {
                trb.data(
                    data_buffer.as_ref().map(|dma| dma.physical()).unwrap_or(0),
                    setup.length,
                    transfer_kind == TransferKind::In,
                    cycle,
                );
                ControlFlow::Break
            },
        ).await?;
        Ok(())
    }
    fn port_req_init_st(&self, port_num: usize, req: &PortReq) -> Result<PortReqState> {
        use usb::setup::*;

        let direction = ReqDirection::from(req.direction);
        let ty = ReqType::from(req.req_type) as u8;
        let recipient = ReqRecipient::from(req.req_recipient) as u8;

        let transfer_kind = match direction {
            _ if !req.transfers_data => TransferKind::NoData,
            ReqDirection::DeviceToHost => TransferKind::In,
            ReqDirection::HostToDevice => TransferKind::Out,
        };

        let setup = Setup {
            kind: ((direction as u8) << USB_SETUP_DIR_SHIFT)
                | (ty << USB_SETUP_REQ_TY_SHIFT)
                | (recipient << USB_SETUP_RECIPIENT_SHIFT),
            request: req.request,
            value: req.value,
            index: req.index,
            length: req.length,
        };
        // TODO: Reuse buffers, or something.
        // TODO: Validate the size.
        // TODO: Sizes above 65536, *perhaps*.
        let data_buffer = unsafe { Dma::<[u8]>::zeroed_unsized(req.length as usize)? };
        assert_eq!(data_buffer.len(), req.length as usize);

        Ok(match transfer_kind {
            TransferKind::In => PortReqState::WaitingForDeviceBytes(data_buffer, setup),
            TransferKind::Out => PortReqState::WaitingForHostBytes(data_buffer, setup),
            TransferKind::NoData => PortReqState::TmpSetup(setup),
            _ => unreachable!(),
        })
        // FIXME: Make sure there aren't any other PortReq handles, perhaps by storing the state in
        // PortState?
    }
    async fn handle_port_req_write(
        &self,
        fd: usize,
        port_num: usize,
        mut st: PortReqState,
        buf: &[u8],
    ) -> Result<usize> {
        let bytes_written = match st {
            PortReqState::Init => {
                let req = serde_json::from_slice::<PortReq>(buf).or(Err(Error::new(EBADMSG)))?;

                st = self.port_req_init_st(port_num, &req)?;

                if let PortReqState::TmpSetup(setup) = st {
                    // No need for any additional reads or writes, before completing.
                    self.port_req_transfer(port_num, None, setup, TransferKind::NoData).await?;
                    st = PortReqState::Init;
                }

                buf.len()
            }
            PortReqState::WaitingForHostBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                dma_buffer.copy_from_slice(buf);

                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::Out).await?;
                st = PortReqState::Init;

                buf.len()
            }
            PortReqState::WaitingForDeviceBytes(_, _) => return Err(Error::new(EBADF)),
            PortReqState::Tmp | PortReqState::TmpSetup(_) => unreachable!(),
        };
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        match &mut *guard {
            Handle::PortReq(_, ref mut state) => *state = st,
            _ => unreachable!(),
        }
        Ok(bytes_written)
    }
    async fn handle_port_req_read(
        &self,
        fd: usize,
        port_num: usize,
        mut st: PortReqState,
        buf: &mut [u8],
    ) -> Result<usize> {
        let bytes_read = match st {
            PortReqState::WaitingForDeviceBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::In).await?;
                buf.copy_from_slice(&dma_buffer);

                st = PortReqState::Init;

                buf.len()
            }
            PortReqState::Init | PortReqState::WaitingForHostBytes(_, _) => {
                return Err(Error::new(EBADF))
            }
            PortReqState::Tmp | PortReqState::TmpSetup(_) => unreachable!(),
        };

        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        match &mut *guard {
            Handle::PortReq(_, ref mut state) => *state = st,
            _ => unreachable!(),
        }
        Ok(bytes_read)
    }
}

impl Scheme for Xhci {
    fn open(&self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid != 0 {
            return Err(Error::new(EACCES));
        }

        let path_str = str::from_utf8(path)
            .or(Err(Error::new(ENOENT)))?
            .trim_start_matches('/');

        let components = path::Path::new(path_str)
            .components()
            .map(|component| -> Option<_> {
                match component {
                    path::Component::Normal(n) => Some(n.to_str()?),
                    _ => None,
                }
            })
            .collect::<Option<SmallVec<[&str; 4]>>>()
            .ok_or(Error::new(ENOENT))?;

        let handle = match &components[..] {
            &[] => {
                if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                    let mut contents = Vec::new();

                    let ports_guard = self.ports.lock().unwrap();

                    for (index, _) in ports_guard
                        .iter()
                        .enumerate()
                        .filter(|(_, port)| port.flags().contains(port::PortFlags::PORT_CCS))
                    {
                        write!(contents, "port{}\n", index).unwrap();
                    }

                    Handle::TopLevel(0, contents)
                } else {
                    return Err(Error::new(EISDIR));
                }
            }
            &[port, port_tl] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                if !self.port_states.contains_key(&port_num) {
                    return Err(Error::new(ENOENT));
                }

                match port_tl {
                    "descriptors" => {
                        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                            return Err(Error::new(ENOTDIR));
                        }

                        let contents = self.port_desc_json(port_num)?;
                        Handle::PortDesc(port_num, 0, contents)
                    }
                    "configure" => {
                        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                            return Err(Error::new(ENOTDIR));
                        }
                        if flags & O_RDWR != O_WRONLY && flags & O_STAT == 0 {
                            return Err(Error::new(EACCES));
                        }

                        Handle::ConfigureEndpoints(port_num)
                    }
                    "state" => {
                        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                            return Err(Error::new(ENOTDIR));
                        }

                        Handle::PortState(port_num, 0)
                    }
                    "request" => {
                        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                            return Err(Error::new(ENOTDIR));
                        }
                        Handle::PortReq(port_num, PortReqState::Init)
                    }
                    "endpoints" => {
                        if flags & O_DIRECTORY == 0 && flags & O_STAT == 0 {
                            return Err(Error::new(EISDIR));
                        };
                        let mut contents = Vec::new();
                        let ps = self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?;

                        /*for (ep_num, _) in self.dev_ctx.contexts[ps.slot as usize].endpoints.iter().enumerate().filter(|(_, ep)| ep.a.read() & 0b111 == 1) {
                            write!(contents, "{}\n", ep_num).unwrap();
                        }*/

                        for ep_num in ps.endpoint_states.keys() {
                            write!(contents, "{}\n", ep_num).unwrap();
                        }

                        Handle::Endpoints(port_num, 0, contents)
                    }
                    _ => return Err(Error::new(ENOENT)),
                }
            }
            &[port, "endpoints", endpoint_num_str] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let endpoint_num = endpoint_num_str.parse::<u8>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY == 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                }

                let port_state = self
                    .port_states
                    .get_mut(&port_num)
                    .ok_or(Error::new(ENOENT))?;

                /*if self.dev_ctx.contexts[port_state.slot as usize].endpoints.get(endpoint_num as usize).ok_or(Error::new(ENOENT))?.a.read() & 0b111 != 1 {
                    return Err(Error::new(ENXIO)); // TODO: Find a proper error code for "endpoint not initialized".
                }*/
                if !port_state.endpoint_states.contains_key(&endpoint_num) {
                    return Err(Error::new(ENOENT));
                }
                let contents = "ctl\ndata\n".as_bytes().to_owned();

                Handle::Endpoint(port_num, endpoint_num, EndpointHandleTy::Root(0, contents))
            }
            &[port, "endpoints", endpoint_num_str, sub] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let endpoint_num = endpoint_num_str.parse::<u8>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                }

                let port_state = self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?;

                if port_state.endpoint_states.get(&endpoint_num).is_none() {
                    return Err(Error::new(ENOENT));
                }

                let st = match sub {
                    "ctl" => EndpointHandleTy::Ctl,
                    "data" => EndpointHandleTy::Data,
                    _ => return Err(Error::new(ENOENT)),
                };
                Handle::Endpoint(port_num, endpoint_num, st)
            }
            &[port] if port.starts_with("port") => {
                if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                    let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                    let mut contents = Vec::new();

                    write!(contents, "descriptors\nendpoints\n").unwrap();

                    if self.slot_state(
                        self.port_states
                            .get(&port_num)
                            .ok_or(Error::new(ENOENT))?
                            .slot as usize,
                    ) != SlotState::Configured as u8
                    {
                        write!(contents, "configure\n").unwrap();
                    }

                    Handle::Port(port_num, 0, contents)
                } else {
                    return Err(Error::new(EISDIR));
                }
            }
            _ => return Err(Error::new(ENOENT)),
        };

        let fd = self.next_handle.fetch_add(1, atomic::Ordering::Relaxed);
        self.handles.insert(fd, handle);

        Ok(fd)
    }

    fn fstat(&self, id: usize, stat: &mut Stat) -> Result<usize> {
        let mut guard = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        match &*guard {
            &Handle::TopLevel(_, ref buf)
            | &Handle::Port(_, _, ref buf)
            | &Handle::Endpoints(_, _, ref buf) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = buf.len() as u64;
            }
            &Handle::PortDesc(_, _, ref buf) => {
                stat.st_mode = MODE_FILE;
                stat.st_size = buf.len() as u64;
            }
            &Handle::PortReq(_, PortReqState::WaitingForDeviceBytes(ref buf, _))
            | &Handle::PortReq(_, PortReqState::WaitingForHostBytes(ref buf, _)) => {
                stat.st_mode = MODE_CHR;
                stat.st_size = buf.len() as u64;
            }
            &Handle::PortReq(_, PortReqState::Tmp)
            | &Handle::PortReq(_, PortReqState::TmpSetup(_)) => unreachable!(),

            &Handle::PortState(_, _) | &Handle::PortReq(_, _) => stat.st_mode = MODE_CHR,
            &Handle::Endpoint(_, _, ref st) => match st {
                &EndpointHandleTy::Ctl | &EndpointHandleTy::Data => stat.st_mode = MODE_CHR,
                &EndpointHandleTy::Root(_, ref buf) => {
                    stat.st_mode = MODE_DIR;
                    stat.st_size = buf.len() as u64;
                }
            },
            &Handle::ConfigureEndpoints(_) => {
                stat.st_mode = MODE_CHR | 0o200; // write only
            }
        }
        Ok(0)
    }

    fn fpath(&self, fd: usize, buffer: &mut [u8]) -> Result<usize> {
        let mut cursor = io::Cursor::new(buffer);

        let guard = self.handles.get(&fd).ok_or(Error::new(EBADF))?;
        match &*guard {
            &Handle::TopLevel(_, _) => write!(cursor, "/").unwrap(),
            &Handle::Port(port_num, _, _) => write!(cursor, "/port{}/", port_num).unwrap(),
            &Handle::PortDesc(port_num, _, _) => {
                write!(cursor, "/port{}/descriptors", port_num).unwrap()
            }
            &Handle::PortState(port_num, _) => write!(cursor, "/port{}/state", port_num).unwrap(),
            &Handle::PortReq(port_num, _) => write!(cursor, "/port{}/request", port_num).unwrap(),
            &Handle::Endpoints(port_num, _, _) => {
                write!(cursor, "/port{}/endpoints/", port_num).unwrap()
            }
            &Handle::Endpoint(port_num, endp_num, ref st) => write!(
                cursor,
                "/port{}/endpoints/{}/{}",
                port_num,
                endp_num,
                match st {
                    &EndpointHandleTy::Root(_, _) => "",
                    &EndpointHandleTy::Ctl => "ctl",
                    &EndpointHandleTy::Data => "data",
                }
            )
            .unwrap(),
            &Handle::ConfigureEndpoints(port_num) => {
                write!(cursor, "/port{}/configure", port_num).unwrap()
            }
        }
        let src_len = usize::try_from(cursor.seek(io::SeekFrom::End(0)).unwrap()).unwrap();
        Ok(src_len)
    }

    fn seek(&self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        match &mut *guard {
            // Directories, or fixed files
            Handle::TopLevel(ref mut offset, ref buf)
            | Handle::Port(_, ref mut offset, ref buf)
            | Handle::PortDesc(_, ref mut offset, ref buf)
            | Handle::Endpoints(_, ref mut offset, ref buf)
            | Handle::Endpoint(_, _, EndpointHandleTy::Root(ref mut offset, ref buf)) => {
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, buf.len())),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, buf.len())),
                    SEEK_END => cmp::max(0, cmp::min(buf.len() + pos, buf.len())),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
            Handle::PortState(_, ref mut offset) => {
                match whence {
                    SEEK_SET => *offset = pos,
                    SEEK_CUR => *offset = pos,
                    SEEK_END => *offset = pos,
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
            // Write-once configure or transfer
            Handle::Endpoint(_, _, _) | Handle::ConfigureEndpoints(_) | Handle::PortReq(_, _) => {
                return Err(Error::new(ESPIPE))
            }
        }
    }

    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        match &mut *guard {
            Handle::TopLevel(ref mut offset, ref src_buf)
            | Handle::Port(_, ref mut offset, ref src_buf)
            | Handle::PortDesc(_, ref mut offset, ref src_buf)
            | Handle::Endpoints(_, ref mut offset, ref src_buf)
            | Handle::Endpoint(_, _, EndpointHandleTy::Root(ref mut offset, ref src_buf)) => {
                let max_bytes_to_read = cmp::min(src_buf.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, *offset) - *offset;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
                *offset += bytes_to_read;

                Ok(bytes_to_read)
            }
            Handle::ConfigureEndpoints(_) => return Err(Error::new(EBADF)),

            &mut Handle::Endpoint(port_num, endp_num, ref mut st) => match st {
                EndpointHandleTy::Ctl => self.on_read_endp_ctl(port_num, endp_num, buf),
                EndpointHandleTy::Data => block_on(self.on_read_endp_data(port_num, endp_num, buf)),
                EndpointHandleTy::Root(_, _) => return Err(Error::new(EBADF)),
            },
            &mut Handle::PortState(port_num, ref mut offset) => {
                let ps = self.port_states.get(&port_num).ok_or(Error::new(EBADF))?;
                let state = self
                    .dev_ctx
                    .contexts
                    .get(ps.slot as usize)
                    .ok_or(Error::new(EBADF))?
                    .slot
                    .state();

                let string = match state {
                    0 => Some(PortState::EnabledOrDisabled),
                    1 => Some(PortState::Default),
                    2 => Some(PortState::Addressed),
                    3 => Some(PortState::Configured),
                    _ => None,
                }
                .as_ref()
                .map(PortState::as_str)
                .unwrap_or("unknown")
                .as_bytes();

                Ok(Self::write_dyn_string(string, buf, offset))
            }
            &mut Handle::PortReq(port_num, ref mut st) => {
                let state = std::mem::replace(st, PortReqState::Tmp);
                block_on(self.handle_port_req_read(fd, port_num, state, buf))
            }
        }
    }
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize> {
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;

        match &mut *guard {
            &mut Handle::ConfigureEndpoints(port_num) => {
                block_on(self.configure_endpoints(port_num, buf))?;
                Ok(buf.len())
            }
            &mut Handle::Endpoint(port_num, endp_num, ref ep_file_ty) => match ep_file_ty {
                EndpointHandleTy::Ctl => block_on(self.on_write_endp_ctl(port_num, endp_num, buf)),
                EndpointHandleTy::Data => block_on(self.on_write_endp_data(port_num, endp_num, buf)),
                EndpointHandleTy::Root(_, _) => return Err(Error::new(EBADF)),
            },
            &mut Handle::PortReq(port_num, ref mut st) => {
                let state = std::mem::replace(st, PortReqState::Tmp);
                block_on(self.handle_port_req_write(fd, port_num, state, buf))
            }
            // TODO: Introduce PortReqState::Waiting, which this write call changes to
            // PortReqState::ReadyToWrite when all bytes are written.
            _ => return Err(Error::new(EBADF)),
        }
    }
    fn close(&self, fd: usize) -> Result<usize> {
        if self.handles.remove(&fd).is_none() {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
}
impl Xhci {
    pub fn get_endp_status(&self, port_num: usize, endp_num: u8) -> Result<EndpointStatus> {
        let port_state = self
            .port_states
            .get(&port_num)
            .ok_or(Error::new(EBADFD))?;

        let slot = port_state.slot;

        let endp_desc = port_state
            .dev_desc
            .as_ref().unwrap()
            .config_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .interface_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .endpoints
            .get(endp_num as usize - 1)
            .ok_or(Error::new(EBADFD))?;

        let endp_num_xhc = if endp_num != 0 {
            Self::endp_num_to_dci(endp_num, endp_desc)
        } else {
            1
        };

        let raw = self
            .dev_ctx
            .contexts
            .get(slot as usize)
            .ok_or(Error::new(EBADFD))?
            .endpoints
            .get(endp_num_xhc as usize - 1)
            .ok_or(Error::new(EBADFD))?
            .a
            .read()
            & super::context::ENDPOINT_CONTEXT_STATUS_MASK;

        Ok(match raw {
            0 => EndpointStatus::Disabled,
            1 => EndpointStatus::Enabled,
            2 => EndpointStatus::Halted,
            3 => EndpointStatus::Stopped,
            4 => EndpointStatus::Error,
            _ => return Err(Error::new(EIO)),
        })
    }
    pub async fn on_req_reset_device(
        &self,
        port_num: usize,
        endp_num: u8,
        clear_feature: bool,
    ) -> Result<()> {
        if self.get_endp_status(port_num, endp_num)? != EndpointStatus::Halted {
            return Err(Error::new(EPROTO));
        }
        // Change the endpoint state from anything, but most likely HALTED (otherwise resetting
        // would be quite meaningless), to stopped.
        self.reset_endpoint(port_num, endp_num, false).await?;
        self.restart_endpoint(port_num, endp_num).await?;

        if clear_feature {
            self.device_req_no_data(
                port_num,
                usb::Setup {
                    kind: 0b0000_0010, // endpoint recipient
                    request: 0x01,     // CLEAR_FEATURE
                    value: 0x00,       // ENDPOINT_HALT
                    index: 0,          // TODO: interface num
                    length: 0,
                },
            ).await?;
        }
        Ok(())
    }
    pub async fn restart_endpoint(&self, port_num: usize, endp_num: u8) -> Result<()> {
        let mut port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADFD))?;
        let slot = port_state.slot;

        let mut endpoint_state = port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADFD))?;

        let (has_streams, ring) = match &mut endpoint_state.transfer {
            &mut super::RingOrStreams::Ring(ref mut ring) => (false, ring),
            &mut super::RingOrStreams::Streams(ref mut arr) => {
                (true, arr.rings.get_mut(&1).ok_or(Error::new(EBADFD))?)
            }
        };

        let (cmd, cycle) = ring.next();
        cmd.transfer_no_op(0, false, false, false, cycle);

        let deque_ptr_and_cycle = ring.register();

        let endp_desc = port_state
            .dev_desc
            .as_ref().unwrap()
            .config_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .interface_descs
            .get(0)
            .ok_or(Error::new(EIO))?
            .endpoints
            .get(endp_num as usize - 1)
            .ok_or(Error::new(EBADFD))?;

        let doorbell = if endp_num != 0 {
            let stream_id = 1u16;

            Self::endp_doorbell(
                endp_num,
                endp_desc,
                if has_streams { stream_id } else { 0 },
            )
        } else {
            Self::def_control_endp_doorbell()
        };

        self.dbs.lock().unwrap()[slot as usize].write(doorbell);

        self.set_tr_deque_ptr(port_num, endp_num, deque_ptr_and_cycle).await?;

        Ok(())
    }
    pub fn endp_direction(&self, port_num: usize, endp_num: u8) -> Result<EndpDirection> {
        Ok(self
            .port_states
            .get(&port_num)
            .ok_or(Error::new(EIO))?
            .dev_desc
            .as_ref().unwrap()
            .config_descs
            .first()
            .ok_or(Error::new(EIO))?
            .interface_descs
            .first()
            .ok_or(Error::new(EIO))?
            .endpoints
            .get(endp_num as usize)
            .ok_or(Error::new(EIO))?
            .direction())
    }
    pub fn slot(&self, port_num: usize) -> Result<u8> {
        Ok(self.port_states.get(&port_num).ok_or(Error::new(EIO))?.slot)
    }
    pub async fn set_tr_deque_ptr(
        &self,
        port_num: usize,
        endp_num: u8,
        deque_ptr_and_cycle: u64,
    ) -> Result<()> {
        let port_state = self.port_states.get(&port_num).ok_or(Error::new(EBADFD))?;
        let slot = port_state.slot;

        let (cfg_idx, if_idx) = match (port_state.cfg_idx, port_state.if_idx) {
            (Some(c), Some(i)) => (c, i),
            _ => return Err(Error::new(EIO)),
        };

        let endp_desc = port_state.dev_desc.as_ref().unwrap().config_descs[usize::from(cfg_idx)].interface_descs[usize::from(if_idx)].endpoints.get(usize::from(endp_num)).ok_or(Error::new(EBADFD))?;
        let endp_num_xhc = Self::endp_num_to_dci(endp_num, endp_desc);

        let (event_trb, command_trb) = self.execute_command(|trb, cycle| {
            trb.set_tr_deque_ptr(
                deque_ptr_and_cycle,
                cycle,
                StreamContextType::PrimaryRing,
                1,
                endp_num_xhc,
                slot,
            )
        }).await;
        self.event_handler_finished();

        handle_event_trb("SET_TR_DEQUEUE_PTR", &event_trb, &command_trb)
    }
    pub async fn on_write_endp_ctl(
        &self,
        port_num: usize,
        endp_num: u8,
        buf: &[u8],
    ) -> Result<usize> {
        let mut port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADF))?;

        let ep_if_state = &mut port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADF))?
            .driver_if_state;

        let req = serde_json::from_slice::<XhciEndpCtlReq>(buf).or(Err(Error::new(EBADMSG)))?;
        match req {
            XhciEndpCtlReq::Status => match ep_if_state {
                state @ EndpIfState::Init => *state = EndpIfState::WaitingForStatus,
                other => {
                    return Err(Error::new(EBADF));
                }
            },
            XhciEndpCtlReq::Reset { no_clear_feature } => match ep_if_state {
                EndpIfState::Init => self.on_req_reset_device(port_num, endp_num, !no_clear_feature).await?,
                other => {
                    return Err(Error::new(EBADF));
                }
            },
            XhciEndpCtlReq::Transfer { direction, count } => match ep_if_state {
                state @ EndpIfState::Init => {
                    if direction == XhciEndpCtlDirection::NoData {
                        // Yield the result directly because no bytes have to be sent or received
                        // beforehand.
                        let (completion_code, bytes_transferred, _) =
                            self.transfer(port_num, endp_num - 1, None, PortReqDirection::DeviceToHost).await?;
                        if bytes_transferred > 0 {
                            return Err(Error::new(EIO));
                        }
                        let result = Self::transfer_result(completion_code, 0);

                        let mut port_state = self
                            .port_states
                            .get_mut(&port_num)
                            .ok_or(Error::new(EBADF))?;
                        let new_state = &mut port_state
                            .endpoint_states
                            .get_mut(&endp_num)
                            .ok_or(Error::new(EBADF))?
                            .driver_if_state;
                        *new_state = EndpIfState::WaitingForTransferResult(result)
                    } else {
                        *state = EndpIfState::WaitingForDataPipe {
                            direction,
                            bytes_to_transfer: count,
                            bytes_transferred: 0,
                        };
                    }
                }
                other => {
                    return Err(Error::new(EBADF));
                }
            },
            other => {
                return Err(Error::new(EBADF));
            }
        }
        Ok(buf.len())
    }
    fn transfer_result(completion_code: u8, bytes_transferred: u32) -> PortTransferStatus {
        let kind = if completion_code == TrbCompletionCode::Success as u8 {
            PortTransferStatusKind::Success
        } else if completion_code == TrbCompletionCode::ShortPacket as u8 {
            PortTransferStatusKind::ShortPacket
        } else if completion_code == TrbCompletionCode::Stall as u8 {
            PortTransferStatusKind::Stalled
        } else {
            PortTransferStatusKind::Unknown
        };
        PortTransferStatus {
            kind,
            bytes_transferred,
        }
    }
    pub async fn on_write_endp_data(
        &self,
        port_num: usize,
        endp_num: u8,
        buf: &[u8],
    ) -> Result<usize> {
        let mut port_state = self.port_states.get_mut(&port_num).ok_or(Error::new(EBADFD))?;
        let mut endpoint_state = port_state.endpoint_states.get_mut(&endp_num).ok_or(Error::new(EBADFD))?;

        let ep_if_state = &mut endpoint_state.driver_if_state;

        match ep_if_state {
            &mut EndpIfState::WaitingForDataPipe {
                direction: XhciEndpCtlDirection::Out,
                bytes_to_transfer: total_bytes_to_transfer,
                bytes_transferred,
            } => {
                if buf.len() > total_bytes_to_transfer as usize - bytes_transferred as usize {
                    return Err(Error::new(EINVAL));
                }
                let (completion_code, some_bytes_transferred) =
                    self.transfer_write(port_num, endp_num - 1, buf).await?;
                let result = Self::transfer_result(completion_code, some_bytes_transferred);

                // To avoid having to read from the Ctl interface file, the client should stop
                // invoking further data transfer calls if any single transfer returns fewer bytes
                // than requested.

                let mut port_state = self.port_states.get_mut(&port_num).ok_or(Error::new(EBADFD))?;
                let mut endpoint_state = port_state.endpoint_states.get_mut(&endp_num).ok_or(Error::new(EBADFD))?;
                let ep_if_state = &mut endpoint_state.driver_if_state;

                if let &mut EndpIfState::WaitingForDataPipe {
                    direction: XhciEndpCtlDirection::Out,
                    bytes_to_transfer,
                    ref mut bytes_transferred,
                } = ep_if_state
                {
                    if *bytes_transferred + some_bytes_transferred == bytes_to_transfer || completion_code != TrbCompletionCode::Success as u8 {
                        *ep_if_state = EndpIfState::WaitingForTransferResult(result);
                    } else {
                        *bytes_transferred += some_bytes_transferred;
                    }
                } else {
                    unreachable!()
                }
                Ok(some_bytes_transferred as usize)
            }
            _ => return Err(Error::new(EBADF)),
        }
    }
    pub fn on_read_endp_ctl(
        &self,
        port_num: usize,
        endp_num: u8,
        buf: &mut [u8],
    ) -> Result<usize> {
        let port_state = &mut self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADF))?;

        let ep_if_state = &mut port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADF))?
            .driver_if_state;

        let res: XhciEndpCtlRes = match ep_if_state {
            &mut EndpIfState::Init => XhciEndpCtlRes::Idle,

            state @ &mut EndpIfState::WaitingForStatus => {
                *state = EndpIfState::Init;
                XhciEndpCtlRes::Status(self.get_endp_status(port_num, endp_num)?)
            }
            &mut EndpIfState::WaitingForDataPipe { .. } => XhciEndpCtlRes::Pending,
            &mut EndpIfState::WaitingForTransferResult(status) => {
                *ep_if_state = EndpIfState::Init;
                XhciEndpCtlRes::TransferResult(status)
            }
        };

        let mut cursor = io::Cursor::new(buf);
        serde_json::to_writer(&mut cursor, &res).or(Err(Error::new(EIO)))?;
        Ok(cursor.seek(io::SeekFrom::Current(0)).unwrap() as usize)
    }
    pub async fn on_read_endp_data(
        &self,
        port_num: usize,
        endp_num: u8,
        buf: &mut [u8],
    ) -> Result<usize> {
        let mut port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADF))?;

        let mut ep_if_state = &mut port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADF))?
            .driver_if_state;

        match ep_if_state {
            &mut EndpIfState::WaitingForDataPipe {
                direction: XhciEndpCtlDirection::In,
                bytes_transferred,
                bytes_to_transfer: total_bytes_to_transfer,
            } => {
                if buf.len() > total_bytes_to_transfer as usize - bytes_transferred as usize {
                    return Err(Error::new(EINVAL));
                }

                let (completion_code, some_bytes_transferred) =
                    self.transfer_read(port_num, endp_num - 1, buf).await?;

                // Just as with on_write_endp_data, a client issuing multiple reads must always
                // stop reading if one read returns fewer bytes than expected.

                let result = Self::transfer_result(completion_code, some_bytes_transferred);

                let mut port_state = self
                    .port_states
                    .get_mut(&port_num)
                    .ok_or(Error::new(EBADF))?;

                let mut ep_state = port_state
                    .endpoint_states
                    .get_mut(&endp_num)
                    .ok_or(Error::new(EBADF))?;

                let ep_if_state = &mut ep_state.driver_if_state;

                if let &mut EndpIfState::WaitingForDataPipe {
                    direction: XhciEndpCtlDirection::In,
                    bytes_to_transfer,
                    ref mut bytes_transferred,
                } = ep_if_state
                {
                    if *bytes_transferred + some_bytes_transferred == bytes_to_transfer || completion_code != TrbCompletionCode::Success as u8 {
                        *ep_if_state = EndpIfState::WaitingForTransferResult(result);
                    } else {
                        *bytes_transferred += some_bytes_transferred;
                    }
                } else {
                    unreachable!()
                }
                Ok(some_bytes_transferred as usize)
            }
            _ => return Err(Error::new(EBADF)),
        }
    }
    /// Notifies the xHC that the current event handler has finished, so that new interrupts can be
    /// sent. This is required after each invocation of `Self::execute_command`.
    ///
    /// # Locking
    /// This function locks `Xhci::run`.
    pub fn event_handler_finished(&self) {
        println!("Event handler finished");
        // write 1 to EHB to clear it
        self.run.lock().unwrap().ints[0].erdp.writef(1 << 3, true);
    }
}
pub fn handle_event_trb(name: &str, event_trb: &Trb, command_trb: &Trb) -> Result<()> {
    if event_trb.completion_code() == TrbCompletionCode::Success as u8 {
        Ok(())
    } else {
        println!("{} command (TRB {:?}) failed with event trb {:?}", name, command_trb, event_trb);
        Err(Error::new(EIO))
    }
}
pub fn handle_transfer_event_trb(name: &str, event_trb: &Trb, transfer_trb: &Trb) -> Result<()> {
    if event_trb.completion_code() == TrbCompletionCode::Success as u8 || event_trb.completion_code() == TrbCompletionCode::ShortPacket as u8 {
        Ok(())
    } else {
        println!("{} transfer (TRB {:?}) failed with event trb {:?}", name, transfer_trb, event_trb);
        Err(Error::new(EIO))
    }
}
use std::ops::{Add, Div, Rem};
pub fn div_round_up<T>(a: T, b: T) -> T
where
    T: Add<Output = T> + Div<Output = T> + Rem<Output = T> + PartialEq + From<u8> + Copy,
{
    if a % b != T::from(0u8) {
        a / b + T::from(1u8)
    } else {
        a / b
    }
}
