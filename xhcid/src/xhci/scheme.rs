use std::convert::TryFrom;
use std::io::prelude::*;
use std::{cmp, mem, path, str};

use serde::{Deserialize, Serialize};
use smallvec::{smallvec, SmallVec};

use syscall::io::{Dma, Io};
use syscall::scheme::SchemeMut;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EBADFD, EBADMSG, EEXIST, EINVAL, EIO, EISDIR, ENOENT, ENOSYS,
    ENOTDIR, ENXIO, EOPNOTSUPP, EOVERFLOW, EPERM, ESPIPE, MODE_CHR, MODE_DIR, MODE_FILE, O_CREAT, O_DIRECTORY,
    O_RDONLY, O_RDWR, O_STAT, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET,
};

use super::{port, usb};
use super::{Device, EndpointState, Xhci};

use super::command::CommandRing;
use super::context::{
    InputContext, SlotState, StreamContext, StreamContextArray, ENDPOINT_CONTEXT_STATUS_MASK,
};
use super::doorbell::Doorbell;
use super::operational::OperationalRegs;
use super::ring::Ring;
use super::runtime::RuntimeRegs;
use super::trb::{TransferKind, TrbCompletionCode, TrbType};
use super::usb::endpoint::{EndpointTy, ENDP_ATTR_TY_MASK};

use crate::driver_interface::*;

/// Subdirs of an endpoint
pub enum EndpointHandleTy {
    /// portX/endpoints/Y/transfer. Write calls transfer data to the device, and read calls
    /// transfer data from the device.
    Transfer,

    /// portX/endpoints/Y/status
    Status(usize), // offset

    /// portX/endpoints/Y/
    Root(usize, Vec<u8>), // offset, content
}

pub enum PortReqState {
    Init,
    WaitingForDeviceBytes(Dma<[u8]>, usb::Setup), // buffer, setup params
    WaitingForHostBytes(Dma<[u8]>, usb::Setup), // buffer, setup params
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
impl IfDesc {
    fn new(
        dev: &mut Device,
        desc: usb::InterfaceDescriptor,
        endps: impl IntoIterator<Item = EndpDesc>,
        hid_descs: impl IntoIterator<Item = HidDesc>,
    ) -> Result<Self> {
        Ok(Self {
            alternate_setting: desc.alternate_setting,
            class: desc.class,
            interface_str: if desc.interface_str > 0 {
                Some(dev.get_string(desc.interface_str)?)
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
    fn device_req_no_data(&mut self, port: usize, req: usb::Setup) -> Result<()> {
        let ps = self.port_states.get_mut(&port).ok_or(Error::new(EIO))?;
        let ring = ps
            .endpoint_states
            .get_mut(&0)
            .ok_or(Error::new(EIO))?
            .ring()
            .ok_or(Error::new(EIO))?;

        {
            let (cmd, cycle) = ring.next();
            cmd.setup(
                req,
                TransferKind::NoData,
                cycle,
            );
        }
        {
            let (cmd, cycle) = ring.next();
            cmd.status(false, cycle);
        }
        self.dbs[ps.slot as usize].write(1);

        {
            let event = self.cmd.next_event();
            while event.data.read() == 0 {
                println!("  - Waiting for event");
            }
            let status = event.status.read();
            let control = event.control.read();

            if (status >> 24) != TrbCompletionCode::Success as u32 {
                println!(
                    "DEVICE_REQ ERROR, COMPLETION CODE {:#0x}",
                    (status >> 24)
                );
            }

            println!(
                "DEVICE_REQ EVENT {:#0x} {:#0x} {:#0x}",
                event.data.read(),
                status,
                control
            );
        }

        self.run.ints[0].erdp.write(self.cmd.erdp());

        Ok(())
    }
    fn set_configuration(&mut self, port: usize, config: u8) -> Result<()> {
        self.device_req_no_data(port, usb::Setup::set_configuration(config))
    }
    fn set_interface(&mut self, port: usize, interface_num: u8, alternate_setting: u8) -> Result<()> {
        self.device_req_no_data(port, usb::Setup::set_interface(interface_num, alternate_setting))
    }

    fn configure_endpoints(&mut self, port: usize, json_buf: &[u8]) -> Result<()> {
        let mut req: ConfigureEndpointsReq =
            serde_json::from_slice(json_buf).or(Err(Error::new(EBADMSG)))?;

        if (!self.cap.cic() || !self.op.cie()) && (req.config_desc != 0 || req.interface_desc != None || req.alternate_setting != None) {
            //return Err(Error::new(EOPNOTSUPP));
            req.config_desc = 0;
            req.alternate_setting = None;
            req.interface_desc = None;
        }
        if req.interface_desc.is_some() != req.alternate_setting.is_some() {
            return Err(Error::new(EBADMSG));
        }

        let port_speed_id = self.ports[port].speed();
        // FIXME
        let speed_id = self.lookup_psiv(port as u8, port_speed_id);
        dbg!(speed_id);

        let port_state = self.port_states.get_mut(&port).ok_or(Error::new(ENOENT))?;
        let input_context: &mut Dma<InputContext> = &mut port_state.input_context;

        // Configure the slot context as well, which holds the last index of the endp descs.
        input_context.add_context.write(1);
        input_context.drop_context.write(0);

        const CONTEXT_ENTRIES_MASK: u32 = 0xF800_0000;
        const CONTEXT_ENTRIES_SHIFT: u8 = 27;

        let current_slot_a = input_context.device.slot.a.read();

        let endpoints =
            &port_state.dev_desc.config_descs.get(req.config_desc as usize).ok_or(Error::new(EBADMSG))?.interface_descs[0].endpoints;

        if endpoints.len() >= 31 {
            return Err(Error::new(EIO));
        }

        let new_context_entries = 1 + endpoints.len() as u32;

        input_context.device.slot.a.write(
            (current_slot_a & !CONTEXT_ENTRIES_MASK)
                | ((u32::from(new_context_entries) << CONTEXT_ENTRIES_SHIFT)
                    & CONTEXT_ENTRIES_MASK),
        );
        input_context.control.write((u32::from(req.alternate_setting.unwrap_or(0)) << 16) | (u32::from(req.interface_desc.unwrap_or(0)) << 8) | u32::from(req.config_desc));

        let lec = self.cap.lec();

        for index in 0..endpoints.len() as u8 {
            let endp_num = index + 1;
            let xhc_endp_num = endp_num + 1;

            input_context.add_context.writef(1 << xhc_endp_num, true);

            let endp_ctx = input_context
                .device
                .endpoints
                .get_mut(endp_num as usize)
                .ok_or(Error::new(EIO))?;
            let endp_desc = endpoints.get(index as usize).ok_or(Error::new(EIO))?;

            // TODO: Check if streams are actually supported.
            let max_streams = endp_desc.max_streams();
            let max_psa_size = self.cap.max_psa_size();

            // TODO: Secondary streams.
            let primary_streams = if max_streams != 0 {
                cmp::min(max_streams, max_psa_size)
            } else {
                0
            };
            let linear_stream_array = if primary_streams != 0 { true } else { false };

            // TODO: Interval related fields
            // TODO: Max ESIT payload size.

            // TODO: The max burst size is non-zero for high-speed isoch endpoints. How are the USB2
            // speeds detected?
            //     I presume that USB 3 devices can never be in low/full/high-speed mode, but
            //     always SuperSpeed (gen 1 and 2 etc.).

            let max_burst_size = endp_desc.max_burst();
            let max_packet_size = endp_desc.max_packet_size;

            let mult = endp_desc.isoch_mult(lec);

            let interval = endp_desc.interval;
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

            let ring_ptr = if max_streams != 0 {
                let mut array = StreamContextArray::new(1 << (max_streams + 1))?;

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
                    EndpointState::Ready(super::RingOrStreams::Streams(array)),
                );

                array_ptr
            } else {
                let ring = Ring::new(true)?;
                let ring_ptr = ring.register();

                assert_eq!(
                    ring_ptr & 0xFFFF_FFFF_FFFF_FF81,
                    ring_ptr,
                    "ring pointer not aligned to 16 bytes"
                );

                port_state.endpoint_states.insert(
                    endp_num,
                    EndpointState::Ready(super::RingOrStreams::Ring(ring)),
                );

                ring_ptr
            };

            assert_eq!(primary_streams & 0x1F, primary_streams);

            endp_ctx.a.write(
                u32::from(mult) << 8
                    | u32::from(interval) << 16
                    | u32::from(primary_streams) << 10
                    | u32::from(linear_stream_array) << 15,
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
            endp_ctx.c.write(u32::from(avg_trb_len));
        }

        self.run.ints[0].erdp.write(self.cmd.erdp());

        {
            let (cmd, cycle, event) = self.cmd.next();
            cmd.configure_endpoint(port_state.slot, input_context.physical(), cycle);

            self.dbs[0].write(0);

            while event.data.read() == 0 {
                println!("    - Waiting for event");
            }

            if event.completion_code() != TrbCompletionCode::Success as u8
                || event.trb_type() != TrbType::CommandCompletion as u8
            {
                println!("CONFIGURE_ENDPOINT failed with event TRB ({:#0x} {:#0x} {:#0x}) and command TRB ({:#0x} {:#0x} {:#0x})", event.data.read(), event.status.read(), event.control.read(), cmd.data.read(), cmd.status.read(), cmd.control.read());
                return Err(Error::new(EIO));
            }

            cmd.reserved(false);
            event.reserved(false);
        }

        // Tell the device about this configuration.

        let configuration_value = port_state
            .dev_desc
            .config_descs
            .get(req.config_desc as usize)
            .ok_or(Error::new(EIO))?
            .configuration_value;
        self.set_configuration(port, configuration_value)?;

        if let (Some(interface_num), Some(alternate_setting)) = (req.interface_desc, req.alternate_setting) {
            self.set_interface(port, interface_num, alternate_setting)?;
        }

        Ok(())
    }
    fn transfer_read(&mut self, port_num: usize, endp_idx: u8, buf: &mut [u8]) -> Result<u32> {
        self.transfer(port_num, endp_idx, if !buf.is_empty() { DeviceReqData::In(buf) } else { DeviceReqData::NoData })
    }
    fn transfer_write(&mut self, port_num: usize, endp_idx: u8, buf: &[u8]) -> Result<u32> {
        self.transfer(port_num, endp_idx, if !buf.is_empty() { DeviceReqData::Out(buf) } else { DeviceReqData::NoData })
    }
    // TODO: Rename DeviceReqData to something more general.
    fn transfer(&mut self, port_num: usize, endp_idx: u8, mut buf: DeviceReqData) -> Result<u32> {
        let endp_num = endp_idx + 1;
        let xhc_endp_num = endp_num + 1;
        // TODO: Check that buf has a nonzero size, otherwise (at least for Rust's GlobalAlloc),
        // UB.
        let dma_buffer = match buf {
            DeviceReqData::Out(sbuf) => {
                let mut dma_buffer = unsafe { Dma::<[u8]>::zeroed_unsized(sbuf.len()) }?;
                dma_buffer.copy_from_slice(sbuf);
                Some(dma_buffer)
            }
            DeviceReqData::In(ref dbuf) => {
                Some(unsafe { Dma::<[u8]>::zeroed_unsized(dbuf.len()) }?)
            }
            DeviceReqData::NoData => None,
        };

        let port_state = self.port_states.get_mut(&port_num).ok_or(Error::new(EBADFD))?;
        let endp_desc: &EndpDesc = port_state.dev_desc.config_descs.get(0).ok_or(Error::new(EIO))?.interface_descs.get(0).ok_or(Error::new(EIO))?.endpoints.get(endp_idx as usize).ok_or(Error::new(EBADFD))?;

        if endp_desc.is_isoch() {
            return Err(Error::new(ENOSYS));
        }

        if EndpDirection::from(buf.direction()) != endp_desc.direction() {
            dbg!(buf.direction(), endp_desc, endp_idx, endp_num);
            return Err(Error::new(EBADF));
        }

        let endp_state = port_state.endpoint_states.get_mut(&endp_num).ok_or(Error::new(EBADF))?;

        let ring: &mut Ring = match endp_state {
            EndpointState::Ready(super::RingOrStreams::Ring(ref mut ring)) => ring,
            EndpointState::Ready(super::RingOrStreams::Streams(stream_ctx_array)) => stream_ctx_array.rings.get_mut(&1).ok_or(Error::new(EBADF))?,
            EndpointState::Init => return Err(Error::new(EIO)),
        };
        // TODO: Scatter-gather transfers, possibly allowing >64KiB sizes.
        let len = u16::try_from(buf.len()).or(Err(Error::new(ENOSYS)))?;
        let max_packet_size = endp_desc.max_packet_size;
        {
            let (trb, cycle) = ring.next();
            let (buffer, idt) = if len <= 8 && max_packet_size >= 8 {
                buf.map_buf(|sbuf| {
                    let mut bytes = [0u8; 8];
                    bytes[..len as usize].copy_from_slice(&sbuf[..len as usize]);
                    // FIXME: little endian, right?
                    (u64::from_le_bytes(bytes), true)
                }).unwrap_or((0, false))
            } else {
                (dma_buffer.as_ref().map(|dma| dma.physical()).unwrap_or(0) as u64, false)
            };
            let estimated_td_size = mem::size_of_val(&trb) as u8; // one trb per td
            trb.normal(buffer, len, cycle, estimated_td_size, false, true, false, true, idt, false);
        }

        let stream_id = 1u16;
        self.dbs[port_state.slot as usize].write(u32::from(xhc_endp_num) | (u32::from(stream_id) << 16));

        let bytes_transferred = {
            let event = self.cmd.next_event();
            while event.data.read() == 0 {
                println!("  - Waiting for event");
            }

            // FIXME: EDTLA if event data was set
            if event.completion_code() != TrbCompletionCode::ShortPacket as u8 && event.transfer_length() != 0 {
                println!("Event trb yielded a short packet, but all bytes were still transferred");
            }

            if event.completion_code() != TrbCompletionCode::Success as u8 || event.trb_type() != TrbType::Transfer as u8 {
                println!("Custom transfer event failed with {:#0x} {:#0x} {:#0x}", event.data.read(), event.status.read(), event.control.read());
            }
            // TODO: Handle event data
            println!("EVENT DATA: {:?}", event.event_data());

            u32::from(len) - event.transfer_length()
        };

        if let DeviceReqData::In(dbuf) = &mut buf {
            dbuf.copy_from_slice(&*dma_buffer.as_ref().unwrap());
        }

        Ok(bytes_transferred)
    }
    pub(crate) fn get_dev_desc(&mut self, port_id: usize) -> Result<DevDesc> {
        let st = self
            .port_states
            .get_mut(&port_id)
            .ok_or(Error::new(ENOENT))?;
        Self::get_dev_desc_raw(
            &mut self.ports,
            &mut self.run,
            &mut self.cmd,
            &mut self.dbs,
            port_id,
            st.slot,
            st.endpoint_states
                .get_mut(&0)
                .ok_or(Error::new(EIO))?
                .ring()
                .ok_or(Error::new(EIO))?,
        )
    }
    pub(crate) fn get_dev_desc_raw(
        ports: &mut [port::Port],
        run: &mut RuntimeRegs,
        cmd: &mut CommandRing,
        dbs: &mut [Doorbell],
        port_id: usize,
        slot: u8,
        ring: &mut Ring,
    ) -> Result<DevDesc> {
        let port = ports.get(port_id).ok_or(Error::new(ENOENT))?;
        if !port.flags().contains(port::PortFlags::PORT_CCS) {
            return Err(Error::new(ENOENT));
        }

        // TODO: Should the descriptors be stored in PortState?

        run.ints[0].erdp.write(cmd.erdp());

        let mut dev = Device {
            ring,
            cmd,
            db: &mut dbs[slot as usize],
            int: &mut run.ints[0],
        };

        let raw_dd = dev.get_device()?;

        let (manufacturer_str, product_str, serial_str) = (
            if raw_dd.manufacturer_str > 0 {
                Some(dev.get_string(raw_dd.manufacturer_str)?)
            } else {
                None
            },
            if raw_dd.product_str > 0 {
                Some(dev.get_string(raw_dd.product_str)?)
            } else {
                None
            },
            if raw_dd.serial_str > 0 {
                Some(dev.get_string(raw_dd.serial_str)?)
            } else {
                None
            },
        );

        let (bos_desc, bos_data) = dev.get_bos()?;
        let has_superspeed =
            usb::bos_capability_descs(bos_desc, &bos_data).any(|desc| desc.is_superspeed());

        let config_descs = (0..raw_dd.configurations)
            .map(|index| -> Result<_> {
                let (desc, data) = dev.get_config(index)?;

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

                            if has_superspeed {
                                let next = match iter.next() {
                                    Some(AnyDescriptor::SuperSpeedCompanion(n)) => n,
                                    _ => break,
                                };
                                endp.ssc = Some(SuperSpeedCmp::from(next));
                            }
                            endpoints.push(endp);
                        }

                        interface_descs.push(IfDesc::new(&mut dev, idesc, endpoints, hid_descs)?);
                    } else {
                        // TODO
                        break;
                    }
                }

                Ok(ConfDesc {
                    kind: desc.kind,
                    configuration: if desc.configuration_str > 0 {
                        Some(dev.get_string(desc.configuration_str)?)
                    } else {
                        None
                    },
                    configuration_value: desc.configuration_value,
                    attributes: desc.attributes,
                    max_power: desc.max_power,
                    interface_descs,
                })
            })
            .collect::<Result<SmallVec<_>>>()?;

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
    fn port_desc_json(&mut self, port_id: usize) -> Result<Vec<u8>> {
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
    fn port_req_transfer(&mut self, port_num: usize, data_buffer: Option<&mut Dma<[u8]>>, setup: usb::Setup, transfer_kind: TransferKind) -> Result<()> {
        // TODO: This json format might be too high level, but is useful for debugging,
        // but when actual device-specific drivers are written, a binary format would
        // be better. Maybe something simple like bincode could be used, if a custom binary struct
        // is too much overkill.

        let port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADF))?;

        let ring = match port_state
            .endpoint_states
            .get_mut(&0)
            .ok_or(Error::new(EIO))?
        {
            EndpointState::Ready(super::RingOrStreams::Ring(ref mut ring)) => ring,

            // Control endpoints never use streams
            _ => return Err(Error::new(EIO)),
        };

        {
            let (cmd, cycle) = ring.next();
            cmd.setup(setup, TransferKind::In, cycle);
        }
        if transfer_kind != TransferKind::NoData {
            let (cmd, cycle) = ring.next();

            cmd.data(
                data_buffer.as_ref().map(|dma| dma.physical()).unwrap_or(0),
                setup.length,
                transfer_kind == TransferKind::In,
                cycle,
            );
        }
        {
            let (cmd, cycle) = ring.next();
            cmd.status(transfer_kind == TransferKind::In, cycle);
        }
        self.dbs[port_state.slot as usize].write(1);

        {
            let event = self.cmd.next_event();
            while event.data.read() == 0 {
                println!("  - Waiting for event");
            }
            if event.completion_code() != TrbCompletionCode::Success as u8
                || event.trb_type() != TrbType::Transfer as u8
            {
                println!(
                    "Custom device request failed with EVENT {:#0x} {:#0x} {:#0x}",
                    event.data.read(),
                    event.status.read(),
                    event.control.read()
                );
            }
        }
        Ok(())
    }
    fn port_req_init_st(&mut self, port_num: usize, req: &PortReq) -> Result<PortReqState> {
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
    fn handle_port_req_write(&mut self, fd: usize, port_num: usize, mut st: PortReqState, buf: &[u8]) -> Result<usize> {
        let bytes_written = match st {
            PortReqState::Init => {
                let req = serde_json::from_slice::<PortReq>(buf).or(Err(Error::new(EBADMSG)))?;

                st = self.port_req_init_st(port_num, &req)?;

                if let PortReqState::TmpSetup(setup) = st {
                    // No need for any additional reads or writes, before completing.
                    self.port_req_transfer(port_num, None, setup, TransferKind::NoData)?;
                    st = PortReqState::Init;
                }

                buf.len()
            }
            PortReqState::WaitingForHostBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                dma_buffer.copy_from_slice(buf);

                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::Out)?;
                st = PortReqState::Init;

                buf.len()
            }
            PortReqState::WaitingForDeviceBytes(_, _) => return Err(Error::new(EBADF)),
            PortReqState::Tmp | PortReqState::TmpSetup(_) => unreachable!(),
        };
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::PortReq(_, ref mut state) => *state = st,
            _ => unreachable!(),
        }
        Ok(bytes_written)
    }
    fn handle_port_req_read(&mut self, fd: usize, port_num: usize, mut st: PortReqState, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = match st {
            PortReqState::WaitingForDeviceBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::In)?;
                buf.copy_from_slice(&dma_buffer);

                st = PortReqState::Init;

                buf.len()
            }
            PortReqState::Init | PortReqState::WaitingForHostBytes(_, _) => return Err(Error::new(EBADF)),
            PortReqState::Tmp | PortReqState::TmpSetup(_) => unreachable!(),
        };
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::PortReq(_, ref mut state) => *state = st,
            _ => unreachable!(),
        }
        Ok(bytes_read)
    }
}

impl SchemeMut for Xhci {
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
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

                    for (index, _) in self
                        .ports
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
                let contents = match port_state
                    .endpoint_states
                    .get(&endpoint_num)
                    .ok_or(Error::new(ENOENT))?
                {
                    EndpointState::Ready { .. } if endpoint_num != 0 => "transfer\nstatus\n",
                    EndpointState::Init | EndpointState::Ready { .. } => "status\n",
                }
                .as_bytes()
                .to_owned();

                Handle::Endpoint(port_num, endpoint_num, EndpointHandleTy::Root(0, contents))
            }
            &[port, "endpoints", endpoint_num_str, sub] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let endpoint_num = endpoint_num_str.parse::<u8>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                }

                let port_state = self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?;

                if port_state
                    .endpoint_states
                    .get(&endpoint_num)
                    .is_none()
                {
                    return Err(Error::new(ENOENT));
                }

                let st = match sub {
                    "status" => {
                        // status is read-only
                        if flags & O_RDWR != O_RDONLY && flags & O_STAT == 0 {
                            return Err(Error::new(EACCES));
                        }
                        EndpointHandleTy::Status(0)
                    }
                    "transfer" => {
                        if endpoint_num == 0 {
                            // Don't allow user programs to interface directly with the control
                            // endpoint.
                            return Err(Error::new(ENOENT));
                        }
                        let endp_desc = &port_state.dev_desc.config_descs.get(0).ok_or(Error::new(EIO))?.interface_descs.get(0).ok_or(Error::new(EIO))?.endpoints.get(endpoint_num as usize - 1).ok_or(Error::new(ENOENT))?;
                        match endp_desc.direction() {
                            EndpDirection::Out => if flags & O_RDWR != O_WRONLY && flags & O_STAT != 0 {
                                return Err(Error::new(EACCES));
                            }
                            EndpDirection::In => if flags & O_RDWR != O_RDONLY && flags & O_STAT != 0 {
                                return Err(Error::new(EACCES));
                            }
                            _ => (),
                        }
                        EndpointHandleTy::Transfer
                    }
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

        let fd = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(fd, handle);

        Ok(fd)
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, ref buf)
            | Handle::Port(_, _, ref buf)
            | Handle::Endpoints(_, _, ref buf) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = buf.len() as u64;
            }
            Handle::PortDesc(_, _, ref buf) => {
                stat.st_mode = MODE_FILE;
                stat.st_size = buf.len() as u64;
            }
            Handle::PortReq(_, PortReqState::WaitingForDeviceBytes(ref buf, _)) | Handle::PortReq(_, PortReqState::WaitingForHostBytes(ref buf, _)) => {
                stat.st_mode = MODE_CHR;
                stat.st_size = buf.len() as u64;
            }
            Handle::PortReq(_, PortReqState::Tmp) | Handle::PortReq(_, PortReqState::TmpSetup(_)) => unreachable!(),

            Handle::PortState(_, _) | Handle::PortReq(_, _) => stat.st_mode = MODE_CHR,
            Handle::Endpoint(_, _, st) => match st {
                EndpointHandleTy::Status(_) | EndpointHandleTy::Transfer => stat.st_mode = MODE_CHR,
                EndpointHandleTy::Root(_, ref buf) => {
                    stat.st_mode = MODE_DIR;
                    stat.st_size = buf.len() as u64;
                }
            },
            Handle::ConfigureEndpoints(_) => {
                stat.st_mode = MODE_CHR | 0o200; // write only
            }
        }
        Ok(0)
    }

    fn fpath(&mut self, fd: usize, buffer: &mut [u8]) -> Result<usize> {
        // XXX: write!() should return the length instead of ().
        let mut src = Vec::<u8>::new();
        match self.handles.get(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, _) => write!(src, "/").unwrap(),
            Handle::Port(port_num, _, _) => write!(src, "/port{}/", port_num).unwrap(),
            Handle::PortDesc(port_num, _, _) => {
                write!(src, "/port{}/descriptors", port_num).unwrap()
            }
            Handle::PortState(port_num, _) => write!(src, "/port{}/state", port_num).unwrap(),
            Handle::PortReq(port_num, _) => write!(src, "/port{}/request", port_num).unwrap(),
            Handle::Endpoints(port_num, _, _) => {
                write!(src, "/port{}/endpoints/", port_num).unwrap()
            }
            Handle::Endpoint(port_num, endp_num, st) => write!(
                src,
                "/port{}/endpoints/{}/{}",
                port_num,
                endp_num,
                match st {
                    EndpointHandleTy::Root(_, _) => "",
                    EndpointHandleTy::Status(_) => "status",
                    EndpointHandleTy::Transfer => "transfer",
                }
            )
            .unwrap(),
            Handle::ConfigureEndpoints(port_num) => {
                write!(src, "/port{}/configure", port_num).unwrap()
            }
        }
        let bytes_to_read = cmp::min(src.len(), buffer.len());
        buffer[..bytes_to_read].copy_from_slice(&src[..bytes_to_read]);
        Ok(bytes_to_read)
    }

    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
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
            // Read-only unknown-length status strings
            Handle::Endpoint(_, _, EndpointHandleTy::Status(ref mut offset))
            | Handle::PortState(_, ref mut offset) => {
                *offset = match whence {
                    SEEK_SET => cmp::max(0, pos),
                    SEEK_CUR => cmp::max(0, *offset + pos),
                    SEEK_END => return Err(Error::new(ESPIPE)),
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

    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
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
                EndpointHandleTy::Transfer => {
                    self.transfer_read(port_num, endp_num - 1, buf)?;
                    // TODO: Perhaps transfers with large buffers could be broken up a to smaller
                    // transfers.
                    Ok(buf.len())
                }
                EndpointHandleTy::Status(ref mut offset) => {
                    let ps = self.port_states.get(&port_num).ok_or(Error::new(EBADF))?;
                    let status = self
                        .dev_ctx
                        .contexts
                        .get(ps.slot as usize)
                        .ok_or(Error::new(EBADF))?
                        .endpoints
                        .get(endp_num as usize)
                        .ok_or(Error::new(EBADF))?
                        .a
                        .read()
                        & ENDPOINT_CONTEXT_STATUS_MASK;

                    let string = match status {
                        0 => Some(EndpointStatus::Disabled),
                        1 => Some(EndpointStatus::Enabled),
                        2 => Some(EndpointStatus::Halted),
                        3 => Some(EndpointStatus::Stopped),
                        4 => Some(EndpointStatus::Error),
                        _ => None,
                    }
                    .as_ref()
                    .map(EndpointStatus::as_str)
                    .unwrap_or("unknown")
                    .as_bytes();

                    Ok(Self::write_dyn_string(string, buf, offset))
                }
                EndpointHandleTy::Root(_, _) => unreachable!(),
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
                self.handle_port_req_read(fd, port_num, state, buf)
            }
        }
    }
    fn write(&mut self, fd: usize, buf: &[u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            &mut Handle::ConfigureEndpoints(port_num) => {
                self.configure_endpoints(port_num, buf)?;
                Ok(buf.len())
            }
            &mut Handle::Endpoint(port_num, endp_num, EndpointHandleTy::Transfer) => {
                self.transfer_write(port_num, endp_num - 1, buf)?;
                // TODO
                Ok(buf.len())
            }
            &mut Handle::PortReq(port_num, ref mut st) => {
                let state = std::mem::replace(st, PortReqState::Tmp);
                self.handle_port_req_write(fd, port_num, state, buf)
            }
            // TODO: Introduce PortReqState::Waiting, which this write call changes to
            // PortReqState::ReadyToWrite when all bytes are written.
            _ => return Err(Error::new(EBADF)),
        }
    }
    fn close(&mut self, fd: usize) -> Result<usize> {
        if self.handles.remove(&fd).is_none() {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
}
