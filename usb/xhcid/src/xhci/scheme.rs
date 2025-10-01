//! Provides the File Descriptor Scheme Interface to the XHCI.
//!
//! This file implements the basic unix file operations that are used to interface with the XHCI
//! driver. While an external program could interact with the driver using this interface, a
//! higher-level abstraction can be found in driver_interface.rs. It is recommended that you use
//! the functions in that module to interact with the driver.
//!
//! The XHCI driver has the following set of schemes:
//!
//! port<n>
//! port<n>/configure
//! port<n>/request
//! port<n>/endpoints
//! port<n>/descriptors
//! port<n>/state
//! port<n>/endpoints/<n>
//! port<n>/endpoints/<n>/ctl
//! port<n>/endpoints/<n>/data
use std::convert::TryFrom;
use std::io::prelude::*;
use std::ops::Deref;
use std::sync::atomic;
use std::{cmp, fmt, io, mem, str};

use common::dma::Dma;
use futures::executor::block_on;
use log::{debug, error, info, trace, warn};
use redox_scheme::scheme::SchemeSync;
use smallvec::SmallVec;

use common::io::Io;
use redox_scheme::{CallerCtx, OpenResult};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, Stat, EACCES, EBADF, EBADFD, EBADMSG, EINVAL, EIO, EISDIR, ENOENT, ENOSYS,
    ENOTDIR, EPROTO, ESPIPE, MODE_CHR, MODE_DIR, MODE_FILE, O_DIRECTORY, O_RDWR, O_STAT, O_WRONLY,
    SEEK_CUR, SEEK_END, SEEK_SET,
};

use super::{port, usb};
use super::{EndpointState, PortId, Xhci};

use super::context::{
    SlotState, StreamContextArray, StreamContextType, CONTEXT_32, CONTEXT_64,
    SLOT_CONTEXT_STATE_MASK, SLOT_CONTEXT_STATE_SHIFT,
};
use super::extended::ProtocolSpeed;
use super::irq_reactor::{EventDoorbell, RingId};
use super::ring::Ring;
use super::trb::{TransferKind, Trb, TrbCompletionCode, TrbType};
use super::usb::endpoint::EndpointTy;

use crate::driver_interface::*;
use regex::Regex;

lazy_static! {
    static ref REGEX_PORT_CONFIGURE: Regex = Regex::new(r"^port([\d\.]+)/configure$")
        .expect("Failed to create the regex for the port<n>/configure scheme.");
    static ref REGEX_PORT_ATTACH: Regex = Regex::new(r"^port([\d\.]+)/attach$")
        .expect("Failed to create the regex for the port<n>/attach scheme.");
    static ref REGEX_PORT_DETACH: Regex = Regex::new(r"^port([\d\.]+)/detach$")
        .expect("Failed to create the regex for the port<n>/detach scheme.");
    static ref REGEX_PORT_DESCRIPTORS: Regex = Regex::new(r"^port([\d\.]+)/descriptors$")
        .expect("Failed to create the regex for the port<n>/descriptors");
    static ref REGEX_PORT_STATE: Regex = Regex::new(r"^port([\d\.]+)/state$")
        .expect("Failed to create the regex for the port<n>/state scheme");
    static ref REGEX_PORT_REQUEST: Regex = Regex::new(r"^port([\d\.]+)/request$")
        .expect("Failed to create the regex for the port<n>/request scheme");
    static ref REGEX_PORT_ENDPOINTS: Regex = Regex::new(r"^port([\d\.]+)/endpoints$")
        .expect("Failed to create the regex for the port<n>/endpoints scheme");
    static ref REGEX_PORT_SPECIFIC_ENDPOINT: Regex =
        Regex::new(r"^port([\d\.]+)/endpoints/(\d{1,3})$")
            .expect("Failed to create the regex for the port<n>/endpoints/<n> scheme");
    static ref REGEX_PORT_SUB_ENDPOINT: Regex = Regex::new(
        r"port([\d\.]+)/endpoints/(\d{1,3})/(ctl|data)$"
    )
    .expect("Failed to create the regex for the port<n>/endpoints/<n>/<sub_endpoint> scheme");
    static ref REGEX_TOP_LEVEL: Regex =
        Regex::new(r"^$").expect("Failed to create the regex for the top-level scheme");
}

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
#[derive(Debug)]
pub enum EndpointHandleTy {
    /// portX/endpoints/Y/data. Allows clients to read and write data associated with ctl requests.
    Data,

    /// portX/endpoints/Y/status
    Ctl,

    /// portX/endpoints/Y/
    Root(Vec<u8>), // content
}

#[derive(Clone, Copy, Debug)]
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

/// The Handle to a specific scheme that is returned by an open() operation.
///
/// Contains some information about the data requested via the handle.
#[derive(Debug)]
pub enum Handle {
    TopLevel(Vec<u8>),                      // contents (ports)
    Port(PortId, Vec<u8>),                  // port, contents
    PortDesc(PortId, Vec<u8>),              // port, contents
    PortState(PortId),                      // port
    PortReq(PortId, PortReqState),          // port, state
    Endpoints(PortId, Vec<u8>),             // port, contents
    Endpoint(PortId, u8, EndpointHandleTy), // port, endpoint, state
    ConfigureEndpoints(PortId),             // port
    AttachDevice(PortId),                   // port
    DetachDevice(PortId),                   // port
}

/// The type of handle.
///
/// This is used by fstat() to determine whether to return a:
///     - MODE_DIR
///     - MODE_FILE
///     - MODE_CHR
pub(crate) enum HandleType {
    Directory,
    File,
    Character,
}

/// Parameters to a handle that were extracted from a scheme.
///
/// This structure is used to easily convert a scheme filesystem path to
/// the parameters that we care about when constructing a handle.
#[derive(Debug)]
enum SchemeParameters {
    /// The scheme references the top-level XHCI driver endpoint
    TopLevel,
    /// /port<n>
    Port(PortId), // port number
    /// /port<n>/descriptors
    PortDesc(PortId), // port number
    /// /port<n>/state
    PortState(PortId), // port number
    /// /port<n>/request
    PortReq(PortId), // port number
    /// /port<n>/endpoints
    Endpoints(PortId), // port number
    /// /port<n>/endpoints/<n>/(data|ctl)
    ///
    /// This can also represent
    /// /port<n>/endpoints/<n>
    Endpoint(PortId, u8, String), // port number, endpoint number, handle type
    /// /port<n>/configure
    ConfigureEndpoints(PortId), // port number
    /// /port<n>/attach
    AttachDevice(PortId), // port number
    /// /port<n>/detach
    DetachDevice(PortId), // port number
}

impl Handle {
    /// Converts a handle back into the scheme that generated it.
    ///
    /// This is useful for implementing fpath, as the input parameters for our existing schemes
    /// are generally static for the lifetime of the driver and can easily be retrieved.
    ///
    /// # Returns
    /// - A [String] containing the scheme path that the handle is associated with.
    pub(crate) fn to_scheme(&self) -> String {
        match self {
            Handle::TopLevel(_) => String::from(""),
            Handle::Port(port_num, _) => {
                format!("port{}", port_num)
            }
            Handle::PortDesc(port_num, _) => {
                format!("port{}/descriptors", port_num)
            }
            Handle::PortState(port_num) => {
                format!("port{}/state", port_num)
            }
            Handle::PortReq(port_num, _) => {
                format!("port{}/request", port_num)
            }
            Handle::Endpoints(port_num, _) => {
                format!("port{}/endpoints", port_num)
            }
            Handle::Endpoint(port_num, endpoint_num, handle_type) => match handle_type {
                EndpointHandleTy::Data => {
                    format!("port{}/endpoints/{}/data", port_num, endpoint_num)
                }
                EndpointHandleTy::Ctl => {
                    format!("port{}/endpoints/{}/ctl", port_num, endpoint_num)
                }
                EndpointHandleTy::Root(_) => {
                    format!("port{}/endpoints/{}", port_num, endpoint_num)
                }
            },
            Handle::ConfigureEndpoints(port_num) => {
                format!("port{}/configure", port_num)
            }
            Handle::AttachDevice(port_num) => {
                format!("port{}/attach", port_num)
            }
            Handle::DetachDevice(port_num) => {
                format!("port{}/detach", port_num)
            }
        }
    }

    /// Gets the access mode for this handle
    ///
    /// Handles can be a file, a directory, or a character interface. The mode that we use is
    /// entirely dependent upon the functionality of the scheme endpoint, so this returns the value
    /// that should be associated with that endpoint.
    ///
    /// # Returns
    /// - [HandleType] - The access mode associated with the handle.
    pub(crate) fn get_handle_type(&self) -> HandleType {
        match self {
            &Handle::TopLevel(_) => HandleType::Directory,
            &Handle::Port(_, _) => HandleType::Directory,
            &Handle::Endpoints(_, _) => HandleType::Directory,
            &Handle::PortDesc(_, _) => HandleType::File,
            &Handle::PortReq(_, PortReqState::WaitingForDeviceBytes(_, _)) => HandleType::Character,
            &Handle::PortReq(_, PortReqState::WaitingForHostBytes(_, _)) => HandleType::Character,
            &Handle::PortReq(_, PortReqState::Tmp) => unreachable!(),
            &Handle::PortReq(_, PortReqState::TmpSetup(_)) => unreachable!(),
            &Handle::PortState(_) => HandleType::Character,
            &Handle::PortReq(_, _) => HandleType::Character,
            &Handle::ConfigureEndpoints(_) => HandleType::Character,
            &Handle::AttachDevice(_) => HandleType::Character,
            &Handle::DetachDevice(_) => HandleType::Character,
            &Handle::Endpoint(_, _, ref st) => match st {
                EndpointHandleTy::Data => HandleType::Character,
                EndpointHandleTy::Ctl => HandleType::Character,
                EndpointHandleTy::Root(_) => HandleType::Directory,
            },
        }
    }

    /// Gets the length of the file buffer as returned by fstat in Stat.st_size
    ///
    /// As some of these endpoints did not return a length in the origin code, this
    /// provides an Option<usize>
    ///
    /// # Returns
    /// Either the size of the buffer, or [Option::None] if the buffer does not exist.
    pub(crate) fn get_buf_len(&self) -> Option<usize> {
        match self {
            &Handle::TopLevel(ref buf) => Some(buf.len()),
            &Handle::Port(_, ref buf) => Some(buf.len()),
            &Handle::Endpoints(_, ref buf) => Some(buf.len()),
            &Handle::PortDesc(_, ref buf) => Some(buf.len()),
            &Handle::PortReq(_, PortReqState::WaitingForDeviceBytes(ref buf, _)) => Some(buf.len()),
            &Handle::PortReq(_, PortReqState::WaitingForHostBytes(ref buf, _)) => Some(buf.len()),
            &Handle::PortReq(_, PortReqState::Tmp) => None,
            &Handle::PortReq(_, PortReqState::TmpSetup(_)) => None,
            &Handle::PortState(_) => None,
            &Handle::PortReq(_, _) => None,
            &Handle::ConfigureEndpoints(_) => None,
            &Handle::AttachDevice(_) => None,
            &Handle::DetachDevice(_) => None,
            &Handle::Endpoint(_, _, ref st) => match st {
                EndpointHandleTy::Data => None,
                EndpointHandleTy::Ctl => None,
                EndpointHandleTy::Root(ref buf) => Some(buf.len()),
            },
        }
    }
}

impl SchemeParameters {
    /// This function gets a partially populated handle from a scheme string.
    ///
    /// This function is intended to be used by the driver's 'open' filesystem
    /// hook to determine if the given string value represents a valid scheme
    ///
    /// # Arguments
    /// 'scheme: &[str]' - A scheme in string format.
    ///
    /// # Returns
    /// A [Result] containing:
    /// - A [SchemeParameters] object representing the scheme that was passed, populated with the input parameters
    /// - [ENOENT] if the passed scheme path is not valid for this driver.
    ///
    /// # Notes
    /// ENOENT is returned so that it can easily be forwarded to the caller of open(). It cleans
    /// up the function considerably to be able to use the ? syntax.
    pub fn from_scheme(scheme: &str) -> Result<Self> {
        fn get_string_from_regex(
            rgx: &Regex,
            scheme: &str,
            capture_idx: usize,
        ) -> syscall::Result<String> {
            if let Some(capture_list) = rgx.captures(scheme) {
                if let Some(value) = capture_list.get(capture_idx + 1) {
                    return Ok(value.as_str().to_string());
                }
            }

            Err(Error::new(ENOENT))
        };

        fn get_port_id_from_regex(
            rgx: &Regex,
            scheme: &str,
            capture_idx: usize,
        ) -> syscall::Result<PortId> {
            if let Some(capture_list) = rgx.captures(scheme) {
                if let Some(value) = capture_list.get(capture_idx + 1) {
                    if let Ok(port_id) = value.as_str().parse::<PortId>() {
                        return Ok(port_id);
                    }
                }
            }

            Err(Error::new(ENOENT))
        };

        fn get_u8_from_regex(rgx: &Regex, scheme: &str, capture_idx: usize) -> syscall::Result<u8> {
            if let Some(capture_list) = rgx.captures(scheme) {
                if let Some(value) = capture_list.get(capture_idx + 1) {
                    if let Ok(integer) = value.as_str().parse::<u8>() {
                        return Ok(integer);
                    }
                }
            }

            Err(Error::new(ENOENT))
        };

        //We don't implement From::<&path::Path> because we don't want to make this a part of
        //the public interface. This function does not guarantee that the handle is VALID, only
        //that the scheme is valid. open() will validate the contents of the enumeration instance,
        //and store it if it's valid.

        //Generate the regular expressions for all of our valid schemes.

        //Check if we have a match and either return a partially initialized scheme, OR ENOENT
        if REGEX_PORT_CONFIGURE.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_CONFIGURE, scheme, 0)?;

            Ok(Self::ConfigureEndpoints(port_num))
        } else if REGEX_PORT_ATTACH.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_ATTACH, scheme, 0)?;

            Ok(Self::AttachDevice(port_num))
        } else if REGEX_PORT_DETACH.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_DETACH, scheme, 0)?;

            Ok(Self::DetachDevice(port_num))
        } else if REGEX_PORT_DESCRIPTORS.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_DESCRIPTORS, scheme, 0)?;

            Ok(Self::PortDesc(port_num))
        } else if REGEX_PORT_STATE.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_STATE, scheme, 0)?;

            Ok(Self::PortState(port_num))
        } else if REGEX_PORT_REQUEST.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_REQUEST, scheme, 0)?;

            Ok(Self::PortReq(port_num))
        } else if REGEX_PORT_ENDPOINTS.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_ENDPOINTS, scheme, 0)?;

            Ok(Self::Endpoints(port_num))
        } else if REGEX_PORT_SPECIFIC_ENDPOINT.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_SPECIFIC_ENDPOINT, scheme, 0)?;
            let endpoint_num = get_u8_from_regex(&REGEX_PORT_SPECIFIC_ENDPOINT, scheme, 1)?;

            Ok(Self::Endpoint(port_num, endpoint_num, String::from("root")))
        } else if REGEX_PORT_SUB_ENDPOINT.is_match(scheme) {
            let port_num = get_port_id_from_regex(&REGEX_PORT_SUB_ENDPOINT, scheme, 0)?;
            let endpoint_num = get_u8_from_regex(&REGEX_PORT_SUB_ENDPOINT, scheme, 1)?;
            let handle_type = get_string_from_regex(&REGEX_PORT_SUB_ENDPOINT, scheme, 2)?;

            Ok(Self::Endpoint(port_num, endpoint_num, handle_type))
        } else if REGEX_TOP_LEVEL.is_match(scheme) {
            Ok(Self::TopLevel)
        } else {
            Err(Error::new(ENOENT))
        }
    }
}

#[derive(Clone, Copy)]
struct DmaSliceDbg<'a, T>(&'a Dma<[T]>);

impl<'a, T> fmt::Debug for DmaSliceDbg<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let DmaSliceDbg(dma) = self;

        f.debug_struct("Dma")
            .field("phys_ptr", &(dma.physical() as *const u8))
            .field("virt_ptr", &(dma.deref().as_ptr() as *const u8))
            .field("length", &(dma.len() * mem::size_of::<T>()))
            .finish()
    }
}

impl fmt::Debug for PortReqState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Init => f.debug_struct("PortReqState::Init").finish(),
            Self::WaitingForDeviceBytes(ref dma, setup) => f
                .debug_tuple("PortReqState::WaitingForDeviceBytes")
                .field(&DmaSliceDbg(dma))
                .field(&setup)
                .finish(),
            Self::WaitingForHostBytes(ref dma, setup) => f
                .debug_tuple("PortReqState::WaitingForHostBytes")
                .field(&DmaSliceDbg(dma))
                .field(&setup)
                .finish(),
            Self::TmpSetup(setup) => f
                .debug_tuple("PortReqState::TmpSetup")
                .field(&setup)
                .finish(),
            Self::Tmp => f.debug_struct("PortReqState::Init").finish(),
        }
    }
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

impl<const N: usize> Xhci<N> {
    async fn new_if_desc(
        &self,
        port_id: PortId,
        slot: u8,
        desc: usb::InterfaceDescriptor,
        endps: impl IntoIterator<Item = EndpDesc>,
        hid_descs: impl IntoIterator<Item = HidDesc>,
        lang_id: u16,
    ) -> Result<IfDesc> {
        Ok(IfDesc {
            alternate_setting: desc.alternate_setting,
            class: desc.class,
            interface_str: if desc.interface_str > 0 {
                Some(
                    self.fetch_string_desc(port_id, slot, desc.interface_str, lang_id)
                        .await?,
                )
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
    pub async fn execute_command<F: FnOnce(&mut Trb, bool)>(&self, f: F) -> (Trb, Trb) {
        //TODO: find out why this bit is set earlier!
        if self.interrupt_is_pending(0) {
            debug!("The EHB bit is already set!");
            //self.force_clear_interrupt(0);
        }

        let next_event = {
            let mut command_ring = self.cmd.lock().unwrap();
            let (cmd_index, cycle) = (command_ring.next_index(), command_ring.cycle);

            debug!("Sending command with cycle bit {}", cycle as u8);

            {
                let command_trb = &mut command_ring.trbs[cmd_index];
                f(command_trb, cycle);
            }

            // get the future here before awaiting, to destroy the lock before deadlock
            let command_trb = &command_ring.trbs[cmd_index];
            self.next_command_completion_event_trb(
                &*command_ring,
                command_trb,
                EventDoorbell::new(self, 0, 0),
            )
        };

        let trbs = next_event.await;
        let event_trb = trbs.event_trb;
        let command_trb = trbs.src_trb.expect("Command completion event TRBs shall always have a valid pointer to a valid source command TRB");

        assert_eq!(
            event_trb.trb_type(),
            TrbType::CommandCompletion as u8,
            "The IRQ reactor (or the xHC) gave an invalid event TRB"
        );

        (event_trb, command_trb)
    }
    pub async fn execute_control_transfer<D>(
        &self,
        port_num: PortId,
        setup: usb::Setup,
        tk: TransferKind,
        name: &str,
        mut d: D,
    ) -> Result<Trb>
    where
        D: FnMut(&mut Trb, bool) -> ControlFlow,
    {
        let future = {
            let mut port_state = self.port_state_mut(port_num)?;
            let slot = port_state.slot;

            let mut endpoint_state = port_state
                .endpoint_states
                .get_mut(&0)
                .ok_or(Error::new(EIO))?;

            let ring = endpoint_state.ring().ok_or(Error::new(EIO))?;

            let first_index = ring.next_index();
            let (cmd, cycle) = (&mut ring.trbs[first_index], ring.cycle);
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
            // When the data stage is in, the status stage must be out
            let input = tk != TransferKind::In;
            let ioc = true;
            let ch = false;
            let ent = false;
            cmd.status(interrupter, input, ioc, ch, ent, cycle);

            self.next_transfer_event_trb(
                RingId::default_control_pipe(port_num),
                ring,
                &ring.trbs[first_index],
                &ring.trbs[last_index],
                EventDoorbell::new(self, usize::from(slot), Self::def_control_endp_doorbell()),
            )
        };

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let status_trb = trbs.src_trb.ok_or(Error::new(EIO))?;

        handle_transfer_event_trb("CONTROL_TRANSFER", &event_trb, &status_trb)?;

        //self.event_handler_finished();

        Ok(event_trb)
    }
    /// NOTE: There has to be AT LEAST one successful invocation of `d`, that actually updates the
    /// TRB (it could be a NO-OP in the worst case).
    /// The function is also required to set the Interrupt on Completion flag, or this function
    /// will never complete.
    pub async fn execute_transfer<D>(
        &self,
        port_num: PortId,
        endp_num: u8,
        stream_id: u16,
        name: &str,
        mut d: D,
    ) -> Result<Trb>
    where
        D: FnMut(&mut Trb, bool) -> ControlFlow,
    {
        let endp_idx = endp_num.checked_sub(1).ok_or(Error::new(EIO))?;
        let mut port_state = self.port_state_mut(port_num)?;

        let slot = port_state.slot;

        let (doorbell_data_stream, doorbell_data_no_stream) = {
            let endp_desc = port_state
                .get_endp_desc(endp_idx)
                .ok_or(Error::new(EBADFD))?;

            //TODO: clean this up
            (
                Self::endp_doorbell(endp_num, endp_desc, stream_id),
                Self::endp_doorbell(endp_num, endp_desc, 0),
            )
        };

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
            } => (
                true,
                stream_ctx_array
                    .rings
                    .get_mut(&1)
                    .ok_or(Error::new(EBADF))?,
            ),
        };

        let future = loop {
            let last_index = ring.next_index();
            let (trb, cycle) = (&mut ring.trbs[last_index], ring.cycle);

            match d(trb, cycle) {
                ControlFlow::Break => {
                    break self.next_transfer_event_trb(
                        super::irq_reactor::RingId {
                            port: port_num,
                            endpoint_num: endp_num,
                            stream_id,
                        },
                        ring,
                        //TODO: find first TRB
                        &ring.trbs[last_index],
                        &ring.trbs[last_index],
                        EventDoorbell::new(
                            self,
                            usize::from(slot),
                            if has_streams {
                                doorbell_data_stream
                            } else {
                                doorbell_data_no_stream
                            },
                        ),
                    );
                }
                ControlFlow::Continue => continue,
            }
        };

        drop(port_state);

        let trbs = future.await;
        let event_trb = trbs.event_trb;
        let transfer_trb = trbs.src_trb.ok_or(Error::new(EIO))?;

        handle_transfer_event_trb("EXECUTE_TRANSFER", &event_trb, &transfer_trb)?;

        // FIXME: EDTLA if event data was set
        if event_trb.completion_code() != TrbCompletionCode::ShortPacket as u8
            && event_trb.transfer_length() != 0
        {
            error!("Event trb didn't yield a short packet, but some bytes were not transferred");
            return Err(Error::new(EIO));
        }

        // TODO: Handle event data
        trace!("EVENT DATA: {:?}", event_trb.event_data());

        Ok(event_trb)
    }
    async fn device_req_no_data(&self, port: PortId, req: usb::Setup) -> Result<()> {
        trace!("DEVICE_REQ_NO_DATA port {}, req: {:?}", port, req);

        self.execute_control_transfer(
            port,
            req,
            TransferKind::NoData,
            "DEVICE_REQ_NO_DATA",
            |_, _| ControlFlow::Break,
        )
        .await?;
        Ok(())
    }

    async fn set_configuration(&self, port: PortId, config: u8) -> Result<()> {
        debug!("Setting configuration value {} to port {}", config, port);
        self.device_req_no_data(port, usb::Setup::set_configuration(config))
            .await
    }

    async fn set_interface(
        &self,
        port: PortId,
        interface_num: u8,
        alternate_setting: u8,
    ) -> Result<()> {
        debug!(
            "Setting interface value {} (alternate setting {}) to port {}",
            interface_num, alternate_setting, port
        );
        self.device_req_no_data(
            port,
            usb::Setup::set_interface(interface_num, alternate_setting),
        )
        .await
    }

    async fn reset_endpoint(&self, port_num: PortId, endp_num: u8, tsp: bool) -> Result<()> {
        let endp_idx = endp_num.checked_sub(1).ok_or(Error::new(EIO))?;
        let port_state = self.port_states.get(&port_num).ok_or(Error::new(EBADFD))?;

        let endp_desc = port_state
            .get_endp_desc(endp_idx)
            .ok_or(Error::new(EBADFD))?;
        let endp_num_xhc = Self::endp_num_to_dci(endp_num, endp_desc);

        let slot = self
            .port_states
            .get(&port_num)
            .ok_or(Error::new(EBADF))?
            .slot;

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.reset_endpoint(slot, endp_num_xhc, tsp, cycle);
            })
            .await;
        //self.event_handler_finished();

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
        } else if endp_desc.has_ssp_companion() {
            endp_desc.sspc.as_ref().unwrap().bytes_per_interval
        } else if endp_desc.ssc.is_some() {
            u32::from(endp_desc.ssc.as_ref().unwrap().bytes_per_interval)
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

    fn port_state(
        &self,
        port: PortId,
    ) -> Result<chashmap::ReadGuard<'_, PortId, super::PortState<N>>> {
        self.port_states.get(&port).ok_or(Error::new(EBADF))
    }
    fn port_state_mut(
        &self,
        port: PortId,
    ) -> Result<chashmap::WriteGuard<'_, PortId, super::PortState<N>>> {
        self.port_states.get_mut(&port).ok_or(Error::new(EBADF))
    }

    async fn configure_endpoints_once(
        &self,
        port: PortId,
        req: &ConfigureEndpointsReq,
    ) -> Result<()> {
        let (endp_desc_count, new_context_entries, configuration_value) = {
            let mut port_state = self.port_states.get_mut(&port).ok_or(Error::new(EBADFD))?;

            port_state.cfg_idx = Some(req.config_desc);

            let config_desc = port_state
                .dev_desc
                .as_ref()
                .unwrap()
                .config_descs
                .iter()
                .find(|desc| desc.configuration_value == req.config_desc)
                .ok_or(Error::new(EBADFD))?;

            //TODO: USE ENDPOINTS FROM ALL INTERFACES
            let mut endp_desc_count = 0;
            let mut new_context_entries = 1;
            for if_desc in config_desc.interface_descs.iter() {
                for endpoint in if_desc.endpoints.iter() {
                    endp_desc_count += 1;
                    let entry = Self::endp_num_to_dci(endp_desc_count, endpoint);
                    if entry > new_context_entries {
                        new_context_entries = entry;
                    }
                }
            }
            new_context_entries += 1;

            if endp_desc_count >= 31 {
                warn!("endpoints length {} >= 31", endp_desc_count);
                return Err(Error::new(EIO));
            }

            (
                endp_desc_count,
                new_context_entries,
                config_desc.configuration_value,
            )
        };
        let lec = self.cap.lec();
        let log_max_psa_size = self.cap.max_psa_size();

        let port_speed_id = self.ports.lock().unwrap()[port.root_hub_port_index()].speed();
        let speed_id: &ProtocolSpeed = self.lookup_psiv(port, port_speed_id).ok_or_else(|| {
            warn!("no speed_id");
            Error::new(EIO)
        })?;

        {
            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            let mut input_context = port_state.input_context.lock().unwrap();

            // Configure the slot context as well, which holds the last index of the endp descs.
            input_context.add_context.write(1);
            input_context.drop_context.write(0);

            const CONTEXT_ENTRIES_MASK: u32 = 0xF800_0000;
            const CONTEXT_ENTRIES_SHIFT: u8 = 27;

            const HUB_PORTS_MASK: u32 = 0xFF00_0000;
            const HUB_PORTS_SHIFT: u8 = 24;

            let mut current_slot_a = input_context.device.slot.a.read();
            let mut current_slot_b = input_context.device.slot.b.read();

            // Set context entries
            current_slot_a &= !CONTEXT_ENTRIES_MASK;
            current_slot_a |=
                (u32::from(new_context_entries) << CONTEXT_ENTRIES_SHIFT) & CONTEXT_ENTRIES_MASK;

            // Set hub data
            current_slot_a &= !(1 << 26);
            current_slot_b &= !HUB_PORTS_MASK;
            if let Some(hub_ports) = req.hub_ports {
                current_slot_a |= 1 << 26;
                current_slot_b |= (u32::from(hub_ports) << HUB_PORTS_SHIFT) & HUB_PORTS_MASK;
            }

            input_context.device.slot.a.write(current_slot_a);
            input_context.device.slot.b.write(current_slot_b);

            let control = if self.op.lock().unwrap().cie() {
                (u32::from(req.alternate_setting.unwrap_or(0)) << 16)
                    | (u32::from(req.interface_desc.unwrap_or(0)) << 8)
                    | u32::from(configuration_value)
            } else {
                0
            };
            input_context.control.write(control);
        }

        for endp_idx in 0..endp_desc_count as u8 {
            let endp_num = endp_idx + 1;

            let mut port_state = self.port_states.get_mut(&port).ok_or(Error::new(EBADFD))?;
            let dev_desc = port_state.dev_desc.as_ref().unwrap();
            let endp_desc = port_state.get_endp_desc(endp_idx).ok_or_else(|| {
                warn!("failed to find endpoint {}", endp_idx);
                Error::new(EIO)
            })?;

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
                EndpointTy::Ctrl => {
                    warn!("trying to use control endpoint");
                    return Err(Error::new(EIO)); // only endpoint zero is of type control, and is configured separately with the address device command.
                }
                EndpointTy::Bulk | EndpointTy::Isoch => 3072, // 3 KiB
                EndpointTy::Interrupt => 1024,                // 1 KiB
            };

            assert_eq!(ep_ty & 0x7, ep_ty);
            assert_eq!(mult & 0x3, mult);
            assert_eq!(max_error_count & 0x3, max_error_count);
            assert_ne!(ep_ty, 0); // 0 means invalid.

            let ring_ptr = if usb_log_max_streams.is_some() {
                let mut array =
                    StreamContextArray::new::<N>(self.cap.ac64(), 1 << (primary_streams + 1))?;

                // TODO: Use as many stream rings as needed.
                array.add_ring::<N>(self.cap.ac64(), 1, true)?;
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
                let ring = Ring::new::<N>(self.cap.ac64(), 16, true)?;
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

            let mut input_context = port_state.input_context.lock().unwrap();
            input_context.add_context.writef(1 << endp_num_xhc, true);

            let endp_i = endp_num_xhc as usize - 1;
            input_context.device.endpoints[endp_i].a.write(
                u32::from(mult) << 8
                    | u32::from(primary_streams) << 10
                    | u32::from(linear_stream_array) << 15
                    | u32::from(interval) << 16
                    | u32::from(max_esit_payload_hi) << 24,
            );
            input_context.device.endpoints[endp_i].b.write(
                max_error_count << 1
                    | u32::from(ep_ty) << 3
                    | u32::from(host_initiate_disable) << 7
                    | u32::from(max_burst_size) << 8
                    | u32::from(max_packet_size) << 16,
            );

            input_context.device.endpoints[endp_i]
                .trl
                .write(ring_ptr as u32);
            input_context.device.endpoints[endp_i]
                .trh
                .write((ring_ptr >> 32) as u32);

            input_context.device.endpoints[endp_i]
                .c
                .write(u32::from(avg_trb_len) | (u32::from(max_esit_payload_lo) << 16));

            log::info!("initialized endpoint {}", endp_num);
        }

        {
            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            let slot = port_state.slot;
            let input_context_physical = port_state.input_context.lock().unwrap().physical();

            let (event_trb, command_trb) = self
                .execute_command(|trb, cycle| {
                    trb.configure_endpoint(slot, input_context_physical, cycle)
                })
                .await;

            //self.event_handler_finished();

            handle_event_trb("CONFIGURE_ENDPOINT", &event_trb, &command_trb)?;
        }

        // Tell the device about this configuration.
        self.set_configuration(port, configuration_value).await?;

        Ok(())
    }

    async fn configure_endpoints(&self, port: PortId, json_buf: &[u8]) -> Result<()> {
        let mut req: ConfigureEndpointsReq =
            serde_json::from_slice(json_buf).or(Err(Error::new(EBADMSG)))?;

        info!(
            "Running configure endpoints command, at port {}, request: {:?}",
            port, req
        );

        if req.interface_desc.is_some() != req.alternate_setting.is_some() {
            return Err(Error::new(EBADMSG));
        }

        let already_configured = {
            let port_state = self.port_states.get(&port).ok_or(Error::new(EBADFD))?;
            port_state.cfg_idx == Some(req.config_desc)
        };

        if !already_configured {
            self.configure_endpoints_once(port, &req).await?;
        }

        if let Some(interface_num) = req.interface_desc {
            if let Some(alternate_setting) = req.alternate_setting {
                self.set_interface(port, interface_num, alternate_setting)
                    .await?;
            }
        }

        Ok(())
    }
    async fn transfer_read(
        &self,
        port_num: PortId,
        endp_idx: u8,
        buf: &mut [u8],
    ) -> Result<(u8, u32)> {
        if buf.is_empty() {
            return Err(Error::new(EINVAL));
        }
        let dma_buffer = unsafe { self.alloc_dma_zeroed_unsized(buf.len())? };

        let (completion_code, bytes_transferred, dma_buffer) = self
            .transfer(
                port_num,
                endp_idx,
                Some(dma_buffer),
                PortReqDirection::DeviceToHost,
            )
            .await?;

        buf.copy_from_slice(&*dma_buffer.as_ref().unwrap());
        Ok((completion_code, bytes_transferred))
    }
    async fn transfer_write(
        &self,
        port_num: PortId,
        endp_idx: u8,
        sbuf: &[u8],
    ) -> Result<(u8, u32)> {
        if sbuf.is_empty() {
            return Err(Error::new(EINVAL));
        }
        let mut dma_buffer = unsafe { self.alloc_dma_zeroed_unsized(sbuf.len()) }?;
        dma_buffer.copy_from_slice(sbuf);

        trace!(
            "TRANSFER_WRITE port {} ep {}, buffer at {:p}, size {}, dma buffer {:?}",
            port_num,
            endp_idx + 1,
            sbuf.as_ptr(),
            sbuf.len(),
            DmaSliceDbg(&dma_buffer)
        );

        let (completion_code, bytes_transferred, _) = self
            .transfer(
                port_num,
                endp_idx,
                Some(dma_buffer),
                PortReqDirection::HostToDevice,
            )
            .await?;
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
        port_num: PortId,
        endp_idx: u8,
        dma_buf: Option<Dma<[u8]>>,
        direction: PortReqDirection,
    ) -> Result<(u8, u32, Option<Dma<[u8]>>)> {
        // TODO: Check that only readable enpoints are read, etc.
        let endp_num = endp_idx + 1;

        let mut port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADFD))?;

        let endp_desc: &EndpDesc = port_state
            .get_endp_desc(endp_idx)
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
            let (buffer, idt) = if dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0) <= 8
                && max_packet_size >= 8
                && direction != EndpDirection::In
            {
                dma_buf
                    .as_ref()
                    .map(|sbuf| {
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
                    div_round_up(
                        dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0),
                        max_transfer_size as usize,
                    ) * mem::size_of::<Trb>(),
                )
                .ok()
                .unwrap_or(0x1F),
                0x1F,
            ); // one trb per td
            (buffer, idt, estimated_td_size)
        };

        let stream_id = 1u16;

        let mut bytes_left = dma_buf.as_ref().map(|buf| buf.len()).unwrap_or(0);

        drop(port_state);

        let event = self
            .execute_transfer(
                port_num,
                endp_num,
                stream_id,
                "CUSTOM_TRANSFER",
                |trb, cycle| {
                    let len = cmp::min(bytes_left, max_transfer_size as usize) as u32;

                    // set the interrupt on completion (IOC) flag for the last trb.
                    let ioc = bytes_left <= max_transfer_size as usize;
                    let chain = !ioc;

                    let interrupter = 0;
                    let ent = false;
                    let isp = true;
                    let bei = false;
                    trb.normal(
                        buffer,
                        len,
                        cycle,
                        estimated_td_size,
                        interrupter,
                        ent,
                        isp,
                        chain,
                        ioc,
                        idt,
                        bei,
                    );

                    bytes_left -= len as usize;

                    if bytes_left != 0 {
                        ControlFlow::Continue
                    } else {
                        ControlFlow::Break
                    }
                },
            )
            .await?;
        //self.event_handler_finished();

        let bytes_transferred = dma_buf
            .as_ref()
            .map(|buf| buf.len() as u32 - event.transfer_length())
            .unwrap_or(0);

        Ok((event.completion_code(), bytes_transferred, dma_buf))
    }
    pub async fn get_desc(&self, port_id: PortId, slot: u8) -> Result<DevDesc> {
        let ports = self.ports.lock().unwrap();
        let port = ports
            .get(port_id.root_hub_port_index())
            .ok_or(Error::new(ENOENT))?;
        if !port.flags().contains(port::PortFlags::CCS) {
            return Err(Error::new(ENOENT));
        }

        let raw_dd = self.fetch_dev_desc(port_id, slot).await?;
        log::debug!("port {} slot {} desc {:X?}", port_id, slot, raw_dd);

        // Only fetch language IDs if we need to. Some devices will fail to return this descriptor
        //TODO: also check configurations and interfaces for defined strings?
        let lang_id =
            if raw_dd.manufacturer_str > 0 || raw_dd.product_str > 0 || raw_dd.serial_str > 0 {
                let lang_ids = self.fetch_lang_ids_desc(port_id, slot).await?;
                // Prefer US English, but fall back to first language ID, or zero
                let en_us_id = 0x409;
                if lang_ids.contains(&en_us_id) {
                    en_us_id
                } else {
                    match lang_ids.first() {
                        Some(some) => *some,
                        None => 0,
                    }
                }
            } else {
                0
            };
        log::debug!("port {} using language ID 0x{:04x}", port_id, lang_id);

        let (manufacturer_str, product_str, serial_str) = (
            if raw_dd.manufacturer_str > 0 {
                Some(
                    self.fetch_string_desc(port_id, slot, raw_dd.manufacturer_str, lang_id)
                        .await?,
                )
            } else {
                None
            },
            if raw_dd.product_str > 0 {
                Some(
                    self.fetch_string_desc(port_id, slot, raw_dd.product_str, lang_id)
                        .await?,
                )
            } else {
                None
            },
            if raw_dd.serial_str > 0 {
                Some(
                    self.fetch_string_desc(port_id, slot, raw_dd.serial_str, lang_id)
                        .await?,
                )
            } else {
                None
            },
        );
        log::debug!(
            "manufacturer {:?} product {:?} serial {:?}",
            manufacturer_str,
            product_str,
            serial_str
        );

        //TODO let (bos_desc, bos_data) = self.fetch_bos_desc(port_id, slot).await?;

        let supports_superspeed = false;
        //TODO usb::bos_capability_descs(bos_desc, &bos_data).any(|desc| desc.is_superspeed());
        let supports_superspeedplus = false;
        //TODO usb::bos_capability_descs(bos_desc, &bos_data).any(|desc| desc.is_superspeedplus());

        let mut config_descs = SmallVec::new();

        for index in 0..raw_dd.configurations {
            debug!("Fetching the config descriptor at index {}", index);
            let (desc, data) = self.fetch_config_desc(port_id, slot, index).await?;
            log::debug!(
                "port {} slot {} config {} desc {:X?}",
                port_id,
                slot,
                index,
                desc
            );

            let extra_length = desc.total_length as usize - mem::size_of_val(&desc);
            let data = &data[..extra_length];

            let mut i = 0;
            let mut descriptors = Vec::new();

            while let Some((descriptor, len)) = AnyDescriptor::parse(&data[i..]) {
                descriptors.push(descriptor);
                i += len;
            }

            let mut interface_descs = SmallVec::new();
            let mut iter = descriptors.into_iter().peekable();

            while let Some(item) = iter.next() {
                if let AnyDescriptor::Interface(idesc) = item {
                    let mut endpoints = SmallVec::<[EndpDesc; 4]>::new();
                    let mut hid_descs = SmallVec::<[HidDesc; 1]>::new();

                    while endpoints.len() < idesc.endpoints as usize {
                        let next = match iter.next() {
                            Some(AnyDescriptor::Endpoint(n)) => n,
                            Some(AnyDescriptor::Hid(h)) if idesc.class == 3 => {
                                hid_descs.push(h.into());
                                continue;
                            }
                            Some(unexpected) => {
                                log::warn!("expected endpoint, got {:X?}", unexpected);
                                break;
                            }
                            None => break,
                        };
                        let mut endp = EndpDesc::from(next);

                        loop {
                            match iter.peek() {
                                Some(AnyDescriptor::SuperSpeedCompanion(n)) => {
                                    endp.ssc = Some(SuperSpeedCmp::from(n.clone()));
                                    iter.next().unwrap();
                                }
                                Some(AnyDescriptor::SuperSpeedPlusCompanion(n)) => {
                                    endp.sspc = Some(SuperSpeedPlusIsochCmp::from(n.clone()));
                                    iter.next().unwrap();
                                }
                                _ => break,
                            }
                        }

                        endpoints.push(endp);
                    }

                    interface_descs.push(
                        self.new_if_desc(port_id, slot, idesc, endpoints, hid_descs, lang_id)
                            .await?,
                    );
                } else {
                    log::warn!("expected interface, got {:?}", item);
                    // TODO
                    //break;
                }
            }

            config_descs.push(ConfDesc {
                kind: desc.kind,
                configuration: if desc.configuration_str > 0 {
                    Some(
                        self.fetch_string_desc(port_id, slot, desc.configuration_str, lang_id)
                            .await?,
                    )
                } else {
                    None
                },
                configuration_value: desc.configuration_value,
                attributes: desc.attributes,
                max_power: desc.max_power,
                interface_descs,
            });
        }

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
    fn port_desc_json(&self, port_id: PortId) -> Result<Vec<u8>> {
        let dev_desc = &self
            .port_states
            .get(&port_id)
            .ok_or(Error::new(ENOENT))?
            .dev_desc;
        serde_json::to_vec(dev_desc).or(Err(Error::new(EIO)))
    }
    fn write_dyn_string(string: &[u8], buf: &mut [u8], offset: usize) -> usize {
        let max_bytes_to_read = cmp::min(string.len(), buf.len());
        let bytes_to_read = cmp::max(offset, max_bytes_to_read) - offset;
        buf[..bytes_to_read].copy_from_slice(&string[..bytes_to_read]);

        bytes_to_read
    }
    async fn port_req_transfer(
        &self,
        port_num: PortId,
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
        )
        .await?;
        Ok(())
    }
    fn port_req_init_st(&self, port_num: PortId, req: &PortReq) -> Result<PortReqState> {
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
        let data_buffer_opt = if req.transfers_data {
            let data_buffer = unsafe { self.alloc_dma_zeroed_unsized(req.length as usize)? };
            assert_eq!(data_buffer.len(), req.length as usize);
            Some(data_buffer)
        } else {
            None
        };

        Ok(match transfer_kind {
            TransferKind::In => PortReqState::WaitingForDeviceBytes(
                data_buffer_opt.ok_or(Error::new(EINVAL))?,
                setup,
            ),
            TransferKind::Out => {
                PortReqState::WaitingForHostBytes(data_buffer_opt.ok_or(Error::new(EINVAL))?, setup)
            }
            TransferKind::NoData => PortReqState::TmpSetup(setup),
            _ => unreachable!(),
        })
        // FIXME: Make sure there aren't any other PortReq handles, perhaps by storing the state in
        // PortState?
    }
    async fn handle_port_req_write(
        &self,
        fd: usize,
        port_num: PortId,
        mut st: PortReqState,
        buf: &[u8],
    ) -> Result<usize> {
        let bytes_written = match st {
            PortReqState::Init => {
                let req = serde_json::from_slice::<PortReq>(buf).or(Err(Error::new(EBADMSG)))?;

                st = self.port_req_init_st(port_num, &req)?;

                if let PortReqState::TmpSetup(setup) = st {
                    // No need for any additional reads or writes, before completing.
                    self.port_req_transfer(port_num, None, setup, TransferKind::NoData)
                        .await?;
                    st = PortReqState::Init;
                }

                buf.len()
            }
            PortReqState::WaitingForHostBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                dma_buffer.copy_from_slice(buf);

                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::Out)
                    .await?;
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
        port_num: PortId,
        mut st: PortReqState,
        buf: &mut [u8],
    ) -> Result<usize> {
        let bytes_read = match st {
            PortReqState::WaitingForDeviceBytes(mut dma_buffer, setup) => {
                if buf.len() != dma_buffer.len() {
                    return Err(Error::new(EINVAL));
                }
                self.port_req_transfer(port_num, Some(&mut dma_buffer), setup, TransferKind::In)
                    .await?;
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

    /// Implements open() for the root level scheme
    ///
    /// # Arguments
    /// - 'flags: [usize]' - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either
    ///
    /// - Handle::TopLevel - The file was opened.
    /// - EISDIR           - This is a directory endpoint, but neither O_DIRECTORY nor O_STAT were passed.
    ///
    fn open_handle_top_level(&self, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
            let mut contents = Vec::new();

            let ports_guard = self.ports.lock().unwrap();

            for (index, _) in ports_guard
                .iter()
                .enumerate()
                .filter(|(_, port)| port.flags().contains(port::PortFlags::CCS))
            {
                write!(contents, "port{}\n", index).unwrap();
            }

            Ok(Handle::TopLevel(contents))
        } else {
            Err(Error::new(EISDIR))
        }
    }

    /// implements open() for /port<n>/descriptors
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::PortDesc] - The handle was opened successfully
    /// - [ENOTDIR]          - Directory-specific flags were passed to open(), but this endpoint is not a directory.
    fn open_handle_port_descriptors(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        let contents = self.port_desc_json(port_num)?;
        Ok(Handle::PortDesc(port_num, contents))
    }

    /// implements open() for /port<n>
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num
    /// - [EISDIR]           - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    fn open_handle_port(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        // The != here is unintuitive. You would assume that you could do
        // flags & O_DIRECTORY || flags & O_STAT, but rust doesn't allow
        // you to cast integers to booleans.
        if (flags & O_DIRECTORY != 0) || (flags & O_STAT != 0) {
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

            Ok(Handle::Port(port_num, contents))
        } else {
            Err(Error::new(EISDIR))
        }
    }

    /// implements open() for /port<n>/state
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [ENOTDIR]          - open() was called on this scheme endpoint, but directory-specific flags were passed to open
    fn open_handle_port_state(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        Ok(Handle::PortState(port_num))
    }

    /// implements open() for /port<n>/endpoints
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    fn open_handle_port_endpoints(&self, port_num: PortId, flags: usize) -> Result<Handle> {
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

        Ok(Handle::Endpoints(port_num, contents))
    }

    /// implements open() for /port<n>/endpoints/<n>
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'endpoint_num: [u8]' - The endpoint number to access
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num
    fn open_handle_endpoint_root(
        &self,
        port_num: PortId,
        endpoint_num: u8,
        flags: usize,
    ) -> Result<Handle> {
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

        Ok(Handle::Endpoint(
            port_num,
            endpoint_num,
            EndpointHandleTy::Root(contents),
        ))
    }

    /// implements open() for /port<n>/endpoints/<n>/data and /port<n>/endpoints/<n>/ctl
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'endpoint_num: [u8]' - The endpoint number to access
    /// - 'handle_type: [String]' - The type of the handle
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num, or no endpoint with the given endpoint_num
    fn open_handle_single_endpoint(
        &self,
        port_num: PortId,
        endpoint_num: u8,
        handle_type: String,
        flags: usize,
    ) -> Result<Handle> {
        match handle_type.as_str() {
            "root" => self.open_handle_endpoint_root(port_num, endpoint_num, flags),
            "ctl" | "data" => {
                if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                }

                let port_state = self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?;

                if port_state.endpoint_states.get(&endpoint_num).is_none() {
                    return Err(Error::new(ENOENT));
                }

                let st = match handle_type.as_str() {
                    "ctl" => EndpointHandleTy::Ctl,
                    "data" => EndpointHandleTy::Data,
                    _ => return Err(Error::new(ENOENT)),
                };
                Ok(Handle::Endpoint(port_num, endpoint_num, st))
            }
            _ => panic!(
                "Scheme parser returned an invalid string '{}' for the endpoint handle type",
                handle_type
            ),
        }
    }

    /// implements open() for /port<n>/configure
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'endpoint_num: [u8]' - The endpoint number to access
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num, or no endpoint with the given endpoint_num
    fn open_handle_configure_endpoints(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        if flags & O_RDWR != O_WRONLY && flags & O_STAT == 0 {
            return Err(Error::new(EACCES));
        }

        Ok(Handle::ConfigureEndpoints(port_num))
    }

    /// implements open() for /port<n>/attach
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num, or no endpoint with the given endpoint_num
    fn open_handle_attach_device(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        if flags & O_RDWR != O_WRONLY && flags & O_STAT == 0 {
            return Err(Error::new(EACCES));
        }

        Ok(Handle::AttachDevice(port_num))
    }

    /// implements open() for /port<n>/detach
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [EISDIR]          - open() was called on this scheme endpoint, but no directory-specific flags were passed to open
    /// - [ENOENT]           - The scheme is valid, but there is no port associated with the given port_num, or no endpoint with the given endpoint_num
    fn open_handle_detach_device(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        if flags & O_RDWR != O_WRONLY && flags & O_STAT == 0 {
            return Err(Error::new(EACCES));
        }

        Ok(Handle::DetachDevice(port_num))
    }

    /// implements open() for /port<n>/request
    ///
    /// # Arguments
    /// - 'port_num: [PortId]' - The port number specified in the scheme path
    /// - 'flags: [usize]'    - The flags parameter passed to open()
    ///
    /// # Returns
    /// This function returns a [Result] containing either:
    ///
    /// - [Handle::Port]     - The handle was opened successfully
    /// - [ENOTDIR]          - open() was called on this scheme endpoint, but directory-specific flags were passed to open
    fn open_handle_port_request(&self, port_num: PortId, flags: usize) -> Result<Handle> {
        if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 {
            return Err(Error::new(ENOTDIR));
        }

        Ok(Handle::PortReq(port_num, PortReqState::Init))
    }
}

impl<const N: usize> SchemeSync for &Xhci<N> {
    fn open(&mut self, path_str: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }

        //Parse the scheme, determine if it's in the valid format, return an error if not.
        //This doesn't guarantee that the parameters themselves are valid (i.e. bounded correctly)
        //only that the scheme itself was parseable.
        let scheme_parameters = SchemeParameters::from_scheme(path_str)?;

        //Once we have our scheme parsed into parameters, we can match on those parameters to
        //find the correct routine to open a handle
        let handle = match scheme_parameters {
            SchemeParameters::TopLevel => self.open_handle_top_level(flags)?,
            SchemeParameters::Port(port_number) => self.open_handle_port(port_number, flags)?,
            SchemeParameters::PortDesc(port_number) => {
                self.open_handle_port_descriptors(port_number, flags)?
            }
            SchemeParameters::PortState(port_number) => {
                self.open_handle_port_state(port_number, flags)?
            }
            SchemeParameters::PortReq(port_number) => {
                self.open_handle_port_request(port_number, flags)?
            }
            SchemeParameters::Endpoints(port_number) => {
                self.open_handle_port_endpoints(port_number, flags)?
            }
            SchemeParameters::Endpoint(port_number, endpoint_number, handle_type) => {
                self.open_handle_single_endpoint(port_number, endpoint_number, handle_type, flags)?
            }
            SchemeParameters::ConfigureEndpoints(port_number) => {
                self.open_handle_configure_endpoints(port_number, flags)?
            }
            SchemeParameters::AttachDevice(port_number) => {
                self.open_handle_attach_device(port_number, flags)?
            }
            SchemeParameters::DetachDevice(port_number) => {
                self.open_handle_detach_device(port_number, flags)?
            }
        };

        let fd = self.next_handle.fetch_add(1, atomic::Ordering::Relaxed);

        trace!("OPENED {} to FD={}, handle: {:?}", path_str, fd, handle);

        self.handles.insert(fd, handle);

        Ok(OpenResult::ThisScheme {
            number: fd,
            flags: NewFdFlags::POSITIONED,
        })
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat, _ctx: &CallerCtx) -> Result<()> {
        let guard = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        stat.st_mode = match (&*guard).get_handle_type() {
            HandleType::Directory => MODE_DIR,
            HandleType::File => MODE_FILE,
            HandleType::Character => MODE_CHR,
        };

        stat.st_size = match (&*guard).get_buf_len() {
            None => stat.st_size,
            Some(size) => size as u64,
        };

        //If we have a handle to the configure scheme, we need to mark it as write only.
        match &*guard {
            Handle::ConfigureEndpoints(_) | Handle::AttachDevice(_) | Handle::DetachDevice(_) => {
                stat.st_mode = stat.st_mode | 0o200;
            }
            _ => {}
        }

        Ok(())
    }

    fn fpath(&mut self, fd: usize, buffer: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        let mut cursor = io::Cursor::new(buffer);

        let guard = self.handles.get(&fd).ok_or(Error::new(EBADF))?;
        let scheme = (&*guard).to_scheme();

        write!(cursor, "{}", scheme.as_str()).expect(
            format!(
                "Failed to convert the file descriptor with value {} to the associated file path",
                fd
            )
            .as_str(),
        );

        let src_len = usize::try_from(cursor.seek(io::SeekFrom::End(0)).unwrap()).unwrap();
        Ok(src_len)
    }

    fn read(
        &mut self,
        fd: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let offset = offset as usize;
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        trace!(
            "READ fd={}, handle={:?}, buf=(addr {:p}, length {})",
            fd,
            guard,
            buf.as_ptr(),
            buf.len()
        );
        match &mut *guard {
            Handle::TopLevel(ref src_buf)
            | Handle::Port(_, ref src_buf)
            | Handle::PortDesc(_, ref src_buf)
            | Handle::Endpoints(_, ref src_buf)
            | Handle::Endpoint(_, _, EndpointHandleTy::Root(ref src_buf)) => {
                let max_bytes_to_read = cmp::min(src_buf.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, offset) - offset;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);

                Ok(bytes_to_read)
            }
            Handle::ConfigureEndpoints(_) => Err(Error::new(EBADF)),
            Handle::AttachDevice(_) => Err(Error::new(EBADF)),
            Handle::DetachDevice(_) => Err(Error::new(EBADF)),

            &mut Handle::Endpoint(port_num, endp_num, ref mut st) => match st {
                EndpointHandleTy::Ctl => self.on_read_endp_ctl(port_num, endp_num, buf),
                EndpointHandleTy::Data => block_on(self.on_read_endp_data(port_num, endp_num, buf)),
                EndpointHandleTy::Root(_) => Err(Error::new(EBADF)),
            },
            &mut Handle::PortState(port_num) => {
                let ps = self.port_states.get(&port_num).ok_or(Error::new(EBADF))?;
                let ctx = self
                    .dev_ctx
                    .contexts
                    .get(ps.slot as usize)
                    .ok_or(Error::new(EBADF))?;
                let state = ((ctx.slot.d.read() & SLOT_CONTEXT_STATE_MASK)
                    >> SLOT_CONTEXT_STATE_SHIFT) as u8;

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

                Ok(Xhci::<N>::write_dyn_string(string, buf, offset))
            }
            &mut Handle::PortReq(port_num, ref mut st) => {
                let state = std::mem::replace(st, PortReqState::Tmp);
                drop(guard); // release the lock
                block_on(self.handle_port_req_read(fd, port_num, state, buf))
            }
        }
    }
    fn write(
        &mut self,
        fd: usize,
        buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let mut guard = self.handles.get_mut(&fd).ok_or(Error::new(EBADF))?;
        trace!(
            "WRITE fd={}, handle={:?}, buf=(addr {:p}, length {})",
            fd,
            guard,
            buf.as_ptr(),
            buf.len()
        );

        match &mut *guard {
            &mut Handle::ConfigureEndpoints(port_num) => {
                block_on(self.configure_endpoints(port_num, buf))?;
                Ok(buf.len())
            }
            &mut Handle::AttachDevice(port_num) => {
                //TODO: accept some arguments in buffer?
                block_on(self.attach_device(port_num))?;
                Ok(buf.len())
            }
            &mut Handle::DetachDevice(port_num) => {
                //TODO: accept some arguments in buffer?
                block_on(self.detach_device(port_num))?;
                Ok(buf.len())
            }
            &mut Handle::Endpoint(port_num, endp_num, ref ep_file_ty) => match ep_file_ty {
                EndpointHandleTy::Ctl => block_on(self.on_write_endp_ctl(port_num, endp_num, buf)),
                EndpointHandleTy::Data => {
                    block_on(self.on_write_endp_data(port_num, endp_num, buf))
                }
                EndpointHandleTy::Root(_) => return Err(Error::new(EBADF)),
            },
            &mut Handle::PortReq(port_num, ref mut st) => {
                let state = std::mem::replace(st, PortReqState::Tmp);
                drop(guard); // release the lock
                block_on(self.handle_port_req_write(fd, port_num, state, buf))
            }
            // TODO: Introduce PortReqState::Waiting, which this write call changes to
            // PortReqState::ReadyToWrite when all bytes are written.
            _ => Err(Error::new(EBADF)),
        }
    }
}
impl<const N: usize> Xhci<N> {
    pub fn on_close(&self, fd: usize) {
        self.handles.remove(&fd);
    }

    pub fn get_endp_status(&self, port_num: PortId, endp_num: u8) -> Result<EndpointStatus> {
        let port_state = self.port_states.get(&port_num).ok_or(Error::new(EBADFD))?;

        let slot = port_state.slot;

        let endp_desc = port_state
            .dev_desc
            .as_ref()
            .unwrap()
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
            .endpoints[endp_num_xhc as usize - 1]
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
        port_num: PortId,
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
            )
            .await?;
        }
        Ok(())
    }
    pub async fn restart_endpoint(&self, port_num: PortId, endp_num: u8) -> Result<()> {
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
            .as_ref()
            .unwrap()
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

            Self::endp_doorbell(endp_num, endp_desc, if has_streams { stream_id } else { 0 })
        } else {
            Self::def_control_endp_doorbell()
        };

        self.dbs.lock().unwrap()[slot as usize].write(doorbell);

        self.set_tr_deque_ptr(port_num, endp_num, deque_ptr_and_cycle)
            .await?;

        Ok(())
    }
    pub fn endp_direction(&self, port_num: PortId, endp_num: u8) -> Result<EndpDirection> {
        Ok(self
            .port_states
            .get(&port_num)
            .ok_or(Error::new(EIO))?
            .dev_desc
            .as_ref()
            .unwrap()
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
    pub fn slot(&self, port_num: PortId) -> Result<u8> {
        Ok(self.port_states.get(&port_num).ok_or(Error::new(EIO))?.slot)
    }
    pub async fn set_tr_deque_ptr(
        &self,
        port_num: PortId,
        endp_num: u8,
        deque_ptr_and_cycle: u64,
    ) -> Result<()> {
        let endp_idx = endp_num.checked_sub(1).ok_or(Error::new(EIO))?;
        let port_state = self.port_states.get(&port_num).ok_or(Error::new(EBADFD))?;
        let slot = port_state.slot;

        let endp_desc = port_state
            .get_endp_desc(endp_idx)
            .ok_or(Error::new(EBADFD))?;
        let endp_num_xhc = Self::endp_num_to_dci(endp_num, endp_desc);

        let (event_trb, command_trb) = self
            .execute_command(|trb, cycle| {
                trb.set_tr_deque_ptr(
                    deque_ptr_and_cycle,
                    cycle,
                    StreamContextType::PrimaryRing,
                    1,
                    endp_num_xhc,
                    slot,
                )
            })
            .await;
        //self.event_handler_finished();

        handle_event_trb("SET_TR_DEQUEUE_PTR", &event_trb, &command_trb)
    }
    pub async fn on_write_endp_ctl(
        &self,
        port_num: PortId,
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
                EndpIfState::Init => {
                    self.on_req_reset_device(port_num, endp_num, !no_clear_feature)
                        .await?
                }
                other => {
                    return Err(Error::new(EBADF));
                }
            },
            XhciEndpCtlReq::Transfer { direction, count } => match ep_if_state {
                state @ EndpIfState::Init => {
                    if direction == XhciEndpCtlDirection::NoData {
                        // Yield the result directly because no bytes have to be sent or received
                        // beforehand.
                        let (completion_code, bytes_transferred, _) = self
                            .transfer(port_num, endp_num - 1, None, PortReqDirection::DeviceToHost)
                            .await?;
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
        port_num: PortId,
        endp_num: u8,
        buf: &[u8],
    ) -> Result<usize> {
        let mut port_state = self
            .port_states
            .get_mut(&port_num)
            .ok_or(Error::new(EBADFD))?;
        let mut endpoint_state = port_state
            .endpoint_states
            .get_mut(&endp_num)
            .ok_or(Error::new(EBADFD))?;

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
                drop(port_state);
                let (completion_code, some_bytes_transferred) =
                    self.transfer_write(port_num, endp_num - 1, buf).await?;
                let result = Self::transfer_result(completion_code, some_bytes_transferred);

                // To avoid having to read from the Ctl interface file, the client should stop
                // invoking further data transfer calls if any single transfer returns fewer bytes
                // than requested.

                let mut port_state = self
                    .port_states
                    .get_mut(&port_num)
                    .ok_or(Error::new(EBADFD))?;
                let mut endpoint_state = port_state
                    .endpoint_states
                    .get_mut(&endp_num)
                    .ok_or(Error::new(EBADFD))?;
                let ep_if_state = &mut endpoint_state.driver_if_state;

                if let &mut EndpIfState::WaitingForDataPipe {
                    direction: XhciEndpCtlDirection::Out,
                    bytes_to_transfer,
                    ref mut bytes_transferred,
                } = ep_if_state
                {
                    if *bytes_transferred + some_bytes_transferred == bytes_to_transfer
                        || completion_code != TrbCompletionCode::Success as u8
                    {
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
        port_num: PortId,
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
        port_num: PortId,
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

                drop(port_state);
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
                    if *bytes_transferred + some_bytes_transferred == bytes_to_transfer
                        || completion_code != TrbCompletionCode::Success as u8
                    {
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
        trace!("Event handler finished");
        // write 1 to EHB to clear it
        self.run.lock().unwrap().ints[0]
            .erdp_low
            .writef(1 << 3, true);
    }
}
pub fn handle_event_trb(name: &str, event_trb: &Trb, command_trb: &Trb) -> Result<()> {
    if event_trb.completion_code() == TrbCompletionCode::Success as u8 {
        Ok(())
    } else {
        error!(
            "{} command (TRB {:?}) failed with event trb {:?}",
            name, command_trb, event_trb
        );
        Err(Error::new(EIO))
    }
}
pub fn handle_transfer_event_trb(name: &str, event_trb: &Trb, transfer_trb: &Trb) -> Result<()> {
    if event_trb.completion_code() == TrbCompletionCode::Success as u8
        || event_trb.completion_code() == TrbCompletionCode::ShortPacket as u8
    {
        Ok(())
    } else {
        error!(
            "{} transfer {:?} failed with event {:?}",
            name, transfer_trb, event_trb
        );
        Err(Error::new(EIO))
    }
}
use lazy_static::lazy_static;
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
