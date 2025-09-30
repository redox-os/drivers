use super::context::StreamContextType;
use crate::usb;
use common::io::{Io, Mmio};
use log::trace;
use std::{fmt, mem};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrbType {
    Reserved,
    /* Transfer */
    Normal,
    SetupStage,
    DataStage,
    StatusStage,
    Isoch,
    Link,
    EventData,
    NoOp,
    /* Command */
    EnableSlot,
    DisableSlot,
    AddressDevice,
    ConfigureEndpoint,
    EvaluateContext,
    ResetEndpoint,
    StopEndpoint,
    SetTrDequeuePointer,
    ResetDevice,
    ForceEvent,
    NegotiateBandwidth,
    SetLatencyToleranceValue,
    GetPortBandwidth,
    ForceHeader,
    NoOpCmd,
    /* Reserved */
    GetExtendedProperty,
    SetExtendedProperty,
    Rsv26,
    Rsv27,
    Rsv28,
    Rsv29,
    Rsv30,
    Rsv31,
    /* Events */
    Transfer,
    CommandCompletion,
    PortStatusChange,
    BandwidthRequest,
    Doorbell,
    HostController,
    DeviceNotification,
    MfindexWrap,
    /* Reserved from 40 to 47, vendor devined from 48 to 63 */
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrbCompletionCode {
    Invalid = 0x00,
    Success = 0x01,
    DataBuffer = 0x02,
    BabbleDetected = 0x03,
    UsbTransaction = 0x04,
    Trb = 0x05,
    Stall = 0x06,
    Resource = 0x07,
    Bandwidth = 0x08,
    NoSlotsAvailable = 0x09,
    InvalidStreamType = 0x0A,
    SlotNotEnabled = 0x0B,
    EndpointNotEnabled = 0x0C,
    ShortPacket = 0x0D,
    RingUnderrun = 0x0E,
    RingOverrun = 0x0F,
    VfEventRingFull = 0x10,
    Parameter = 0x11,
    BandwidthOverrun = 0x12,
    ContextState = 0x13,
    NoPingResponse = 0x14,
    EventRingFull = 0x15,
    IncompatibleDevice = 0x16,
    MissedService = 0x17,
    CommandRingStopped = 0x18,
    CommandAborted = 0x19,
    Stopped = 0x1A,
    StoppedLengthInvalid = 0x1B,
    StoppedShortPacket = 0x1C,
    MaxExitLatencyTooLarge = 0x1D,
    Rsv30 = 0x1E,
    IsochBuffer = 0x1F,
    EventLost = 0x20,
    Undefined = 0x21,
    InvalidStreamId = 0x22,
    SecondaryBandwidth = 0x23,
    SplitTransaction = 0x24,
    /* Values from 37 to 191 are reserved */
    /* 192 to 223 are vendor defined errors */
    /* 224 to 255 are vendor defined information */
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferKind {
    NoData,
    Reserved,
    Out,
    In,
}

#[repr(C, packed)]
pub struct Trb {
    pub data_low: Mmio<u32>,
    pub data_high: Mmio<u32>,
    pub status: Mmio<u32>,
    pub control: Mmio<u32>,
}
impl Clone for Trb {
    fn clone(&self) -> Self {
        Self {
            data_low: Mmio::new(self.data_low.read()),
            data_high: Mmio::new(self.data_high.read()),
            status: Mmio::new(self.status.read()),
            control: Mmio::new(self.control.read()),
        }
    }
}

pub const TRB_STATUS_COMPLETION_CODE_SHIFT: u8 = 24;
pub const TRB_STATUS_COMPLETION_CODE_MASK: u32 = 0xFF00_0000;

pub const TRB_STATUS_COMPLETION_PARAM_SHIFT: u8 = 0;
pub const TRB_STATUS_COMPLETION_PARAM_MASK: u32 = 0x00FF_FFFF;

pub const TRB_STATUS_TRANSFER_LENGTH_SHIFT: u8 = 0;
pub const TRB_STATUS_TRANSFER_LENGTH_MASK: u32 = 0x00FF_FFFF;

pub const TRB_CONTROL_TRB_TYPE_SHIFT: u8 = 10;
pub const TRB_CONTROL_TRB_TYPE_MASK: u32 = 0x0000_FC00;

pub const TRB_CONTROL_EVENT_DATA_SHIFT: u8 = 2;
pub const TRB_CONTROL_EVENT_DATA_BIT: u32 = 1 << TRB_CONTROL_EVENT_DATA_SHIFT;

pub const TRB_CONTROL_ENDPOINT_ID_MASK: u32 = 0x001F_0000;
pub const TRB_CONTROL_ENDPOINT_ID_SHIFT: u8 = 16;

impl Trb {
    pub fn set(&mut self, data: u64, status: u32, control: u32) {
        self.data_low.write(data as u32);
        self.data_high.write((data >> 32) as u32);
        self.status.write(status);
        self.control.write(control);
    }

    pub fn reserved(&mut self, cycle: bool) {
        self.set(0, 0, ((TrbType::Reserved as u32) << 10) | (cycle as u32));
    }

    pub fn read_data(&self) -> u64 {
        (self.data_low.read() as u64) | ((self.data_high.read() as u64) << 32)
    }

    pub fn completion_code(&self) -> u8 {
        (self.status.read() >> TRB_STATUS_COMPLETION_CODE_SHIFT) as u8
    }
    pub fn completion_param(&self) -> u32 {
        self.status.read() & TRB_STATUS_COMPLETION_PARAM_MASK
    }
    fn has_completion_trb_pointer(&self) -> bool {
        if self.completion_code() == TrbCompletionCode::RingUnderrun as u8
            || self.completion_code() == TrbCompletionCode::RingOverrun as u8
        {
            false
        } else if self.completion_code() == TrbCompletionCode::VfEventRingFull as u8 {
            false
        } else {
            true
        }
    }
    pub fn completion_trb_pointer(&self) -> Option<u64> {
        debug_assert_eq!(self.trb_type(), TrbType::CommandCompletion as u8);

        if self.has_completion_trb_pointer() {
            Some(self.read_data())
        } else {
            None
        }
    }
    pub fn transfer_event_trb_pointer(&self) -> Option<u64> {
        debug_assert_eq!(self.trb_type(), TrbType::Transfer as u8);

        if self.has_completion_trb_pointer() {
            Some(self.read_data())
        } else {
            None
        }
    }

    pub fn port_status_change_port_id(&self) -> Option<u8> {
        debug_assert_eq!(self.trb_type(), TrbType::PortStatusChange as u8);

        if self.has_completion_trb_pointer() {
            let data = self.read_data();
            Some(((data >> 24) & 0xFF) as u8)
        } else {
            None
        }
    }

    pub fn event_slot(&self) -> u8 {
        (self.control.read() >> 24) as u8
    }
    /// Returns the number of bytes that should have been transmitten, but weren't.
    pub fn transfer_length(&self) -> u32 {
        self.status.read() & TRB_STATUS_TRANSFER_LENGTH_MASK
    }
    pub fn event_data_bit(&self) -> bool {
        self.control.readf(TRB_CONTROL_EVENT_DATA_BIT)
    }
    pub fn event_data(&self) -> Option<u64> {
        if self.event_data_bit() {
            Some(self.read_data())
        } else {
            None
        }
    }
    pub fn endpoint_id(&self) -> u8 {
        ((self.control.read() & TRB_CONTROL_ENDPOINT_ID_MASK) >> TRB_CONTROL_ENDPOINT_ID_SHIFT)
            as u8
    }
    pub fn trb_type(&self) -> u8 {
        ((self.control.read() & TRB_CONTROL_TRB_TYPE_MASK) >> TRB_CONTROL_TRB_TYPE_SHIFT) as u8
    }

    pub fn link(&mut self, address: usize, toggle: bool, cycle: bool) {
        self.set(
            address as u64,
            0,
            ((TrbType::Link as u32) << 10) | ((toggle as u32) << 1) | (cycle as u32),
        );
    }

    pub fn no_op_cmd(&mut self, cycle: bool) {
        self.set(0, 0, ((TrbType::NoOpCmd as u32) << 10) | (cycle as u32));
    }

    pub fn enable_slot(&mut self, slot_type: u8, cycle: bool) {
        trace!("Enabling slot with type {}", slot_type);
        self.set(
            0,
            0,
            (((slot_type as u32) & 0x1F) << 16)
                | ((TrbType::EnableSlot as u32) << 10)
                | (cycle as u32),
        );
    }
    pub fn disable_slot(&mut self, slot: u8, cycle: bool) {
        self.set(
            0,
            0,
            (u32::from(slot) << 24) | ((TrbType::DisableSlot as u32) << 10) | u32::from(cycle),
        );
    }

    pub fn address_device(&mut self, slot_id: u8, input_ctx_ptr: usize, bsr: bool, cycle: bool) {
        assert_eq!(
            (input_ctx_ptr as u64) & 0xFFFF_FFFF_FFFF_FFF0,
            input_ctx_ptr as u64,
            "unaligned input context ptr"
        );
        self.set(
            input_ctx_ptr as u64,
            0,
            (u32::from(slot_id) << 24)
                | ((TrbType::AddressDevice as u32) << 10)
                | (u32::from(bsr) << 9)
                | u32::from(cycle),
        );
    }
    // Synchronizes the input context endpoints with the device context endpoints, I think.
    pub fn configure_endpoint(&mut self, slot_id: u8, input_ctx_ptr: usize, cycle: bool) {
        assert_eq!(
            (input_ctx_ptr as u64) & 0xFFFF_FFFF_FFFF_FFF0,
            input_ctx_ptr as u64,
            "unaligned input context ptr"
        );

        self.set(
            input_ctx_ptr as u64,
            0,
            (u32::from(slot_id) << 24)
                | ((TrbType::ConfigureEndpoint as u32) << 10)
                | u32::from(cycle),
        );
    }
    pub fn evaluate_context(&mut self, slot_id: u8, input_ctx_ptr: usize, bsr: bool, cycle: bool) {
        assert_eq!(
            (input_ctx_ptr as u64) & 0xFFFF_FFFF_FFFF_FFF0,
            input_ctx_ptr as u64,
            "unaligned input context ptr"
        );
        self.set(
            input_ctx_ptr as u64,
            0,
            (u32::from(slot_id) << 24)
                | ((TrbType::EvaluateContext as u32) << 10)
                | (u32::from(bsr) << 9)
                | u32::from(cycle),
        );
    }
    pub fn reset_endpoint(&mut self, slot_id: u8, endp_num_xhc: u8, tsp: bool, cycle: bool) {
        assert_eq!(endp_num_xhc & 0x1F, endp_num_xhc);
        self.set(
            0,
            0,
            (u32::from(slot_id) << 24)
                | (u32::from(endp_num_xhc) << 16)
                | ((TrbType::ResetEndpoint as u32) << 10)
                | (u32::from(tsp) << 9)
                | u32::from(cycle),
        );
    }
    /// The deque_ptr has to contain the DCS bit (bit 0).
    pub fn set_tr_deque_ptr(
        &mut self,
        deque_ptr: u64,
        cycle: bool,
        sct: StreamContextType,
        stream_id: u16,
        endp_num_xhc: u8,
        slot: u8,
    ) {
        assert_eq!(deque_ptr & 0xFFFF_FFFF_FFFF_FFF1, deque_ptr);
        assert_eq!(endp_num_xhc & 0x1F, endp_num_xhc);

        self.set(
            deque_ptr | ((sct as u64) << 1),
            u32::from(stream_id) << 16,
            (u32::from(slot) << 24)
                | (u32::from(endp_num_xhc) << 16)
                | ((TrbType::SetTrDequeuePointer as u32) << 10)
                | u32::from(cycle),
        )
    }
    pub fn stop_endpoint(&mut self, slot_id: u8, endp_num_xhc: u8, suspend: bool, cycle: bool) {
        assert_eq!(endp_num_xhc & 0x1F, endp_num_xhc);
        self.set(
            0,
            0,
            (u32::from(slot_id) << 24)
                | (u32::from(suspend) << 23)
                | (u32::from(endp_num_xhc) << 16)
                | ((TrbType::StopEndpoint as u32) << 10)
                | u32::from(cycle),
        );
    }
    pub fn reset_device(&mut self, slot_id: u8, cycle: bool) {
        self.set(
            0,
            0,
            (u32::from(slot_id) << 24) | ((TrbType::ResetDevice as u32) << 10) | u32::from(cycle),
        );
    }

    pub fn transfer_no_op(&mut self, interrupter: u8, ent: bool, ch: bool, ioc: bool, cycle: bool) {
        self.set(
            0,
            u32::from(interrupter) << 22,
            ((TrbType::NoOp as u32) << 10)
                | (u32::from(ioc) << 5)
                | (u32::from(ch) << 4)
                | (u32::from(ent) << 1)
                | u32::from(cycle),
        );
    }

    pub fn setup(&mut self, setup: usb::Setup, transfer: TransferKind, cycle: bool) {
        self.set(
            unsafe { mem::transmute(setup) },
            8,
            ((transfer as u32) << 16)
                | ((TrbType::SetupStage as u32) << 10)
                | (1 << 6)
                | (cycle as u32),
        );
    }

    pub fn data(&mut self, buffer: usize, length: u16, input: bool, cycle: bool) {
        self.set(
            buffer as u64,
            length as u32,
            ((input as u32) << 16) | ((TrbType::DataStage as u32) << 10) | (cycle as u32),
        );
    }

    pub fn cycle(&self) -> bool {
        self.control.readf(0x01)
    }

    pub fn status(
        &mut self,
        interrupter: u16,
        input: bool,
        ioc: bool,
        ch: bool,
        ent: bool,
        cycle: bool,
    ) {
        self.set(
            0,
            u32::from(interrupter) << 22,
            (u32::from(input) << 16)
                | ((TrbType::StatusStage as u32) << 10)
                | (u32::from(ioc) << 5)
                | (u32::from(ch) << 4)
                | (u32::from(ent) << 1)
                | (cycle as u32),
        );
    }
    pub fn normal(
        &mut self,
        buffer: u64,
        len: u32,
        cycle: bool,
        estimated_td_size: u8,
        interrupter: u8,
        ent: bool,
        isp: bool,
        chain: bool,
        ioc: bool,
        idt: bool,
        bei: bool,
    ) {
        assert_eq!(estimated_td_size & 0x1F, estimated_td_size);
        // NOTE: The interrupter target and no snoop flags have been omitted.
        self.set(
            buffer,
            len | (u32::from(estimated_td_size) << 17) | (u32::from(interrupter) << 22),
            u32::from(cycle)
                | (u32::from(ent) << 1)
                | (u32::from(isp) << 2)
                | (u32::from(chain) << 4)
                | (u32::from(ioc) << 5)
                | (u32::from(idt) << 6)
                | (u32::from(bei) << 9)
                | ((TrbType::Normal as u32) << 10),
        )
    }
    pub fn is_command_trb(&self) -> bool {
        let valid_trb_types = [
            TrbType::NoOpCmd as u8,
            TrbType::EnableSlot as u8,
            TrbType::DisableSlot as u8,
            TrbType::AddressDevice as u8,
            TrbType::ConfigureEndpoint as u8,
            TrbType::EvaluateContext as u8,
            TrbType::ResetEndpoint as u8,
            TrbType::StopEndpoint as u8,
            TrbType::SetTrDequeuePointer as u8,
            TrbType::ResetDevice as u8,
            TrbType::ForceEvent as u8,
            TrbType::NegotiateBandwidth as u8,
            TrbType::SetLatencyToleranceValue as u8,
            TrbType::GetPortBandwidth as u8,
            TrbType::ForceHeader as u8,
            TrbType::GetExtendedProperty as u8,
            TrbType::SetExtendedProperty as u8,
        ];
        valid_trb_types.contains(&self.trb_type())
    }
    pub fn is_transfer_trb(&self) -> bool {
        // XXX: Unfortunately, the only way to use match statements with integer constants, is to
        // precast them into valid enum values, which either requires a derive macro such as
        // num_traits's #[derive(FromPrimitive)], or manually writing the reverse match statement
        // first.
        let valid_trb_types = [
            TrbType::Normal as u8,
            TrbType::SetupStage as u8,
            TrbType::DataStage as u8,
            TrbType::StatusStage as u8,
            TrbType::Isoch as u8,
            TrbType::NoOp as u8,
        ];
        valid_trb_types.contains(&self.trb_type())
    }
}

impl fmt::Debug for Trb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Trb {{ data: {:>016X}, status: {:>08X}, control: {:>08X} }}",
            self.read_data(),
            self.status.read(),
            self.control.read()
        )
    }
}

impl fmt::Display for Trb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "({:>016X}, {:>08X}, {:>08X})",
            self.read_data(),
            self.status.read(),
            self.control.read()
        )
    }
}
