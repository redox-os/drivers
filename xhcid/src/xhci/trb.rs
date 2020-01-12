use std::{fmt, mem};
use syscall::io::{Io, Mmio};
use crate::usb;

#[repr(u8)]
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
    Rsv24,
    Rsv25,
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
pub enum TrbCompletionCode {
    Invalid,
    Success,
    DataBuffer,
    BabbleDetected,
    UsbTransaction,
    Trb,
    Stall,
    Resource,
    Bandwidth,
    NoSlotsAvailable,
    InvalidStreamType,
    SlotNotEnabled,
    EndpointNotEnabled,
    ShortPacket,
    RingUnderrun,
    RingOverrun,
    VfEventRingFull,
    Parameter,
    BandwidthOverrun,
    ContextState,
    NoPingResponse,
    EventRingFull,
    IncompatibleDevice,
    MissedService,
    CommandRingStopped,
    CommandAborted,
    Stopped,
    StoppedLengthInvalid,
    StoppedShortPacket,
    MaxExitLatencyTooLarge,
    Rsv30,
    IsochBuffer,
    EventLost,
    Undefined,
    InvalidStreamId,
    SecondaryBandwidth,
    SplitTransaction,
    /* Values from 37 to 191 are reserved */
    /* 192 to 223 are vendor defined errors */
    /* 224 to 255 are vendor defined information */
}

#[repr(u8)]
pub enum TransferKind {
    NoData,
    Reserved,
    Out,
    In,
}

#[repr(packed)]
pub struct Trb {
    pub data: Mmio<u64>,
    pub status: Mmio<u32>,
    pub control: Mmio<u32>,
}

impl Trb {
    pub fn set(&mut self, data: u64, status: u32, control: u32) {
        self.data.write(data);
        self.status.write(status);
        self.control.write(control);
    }

    pub fn reserved(&mut self, cycle: bool) {
        self.set(
            0,
            0,
            ((TrbType::Reserved as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn link(&mut self, address: usize, toggle: bool, cycle: bool) {
        self.set(
            address as u64,
            0,
            ((TrbType::Link as u32) << 10) |
            ((toggle as u32) << 1) |
            (cycle as u32)
        );
    }

    pub fn no_op_cmd(&mut self, cycle: bool) {
        self.set(
            0,
            0,
            ((TrbType::NoOpCmd as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn enable_slot(&mut self, slot_type: u8, cycle: bool) {
        self.set(
            0,
            0,
            (((slot_type as u32) & 0x1F) << 16) |
            ((TrbType::EnableSlot as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn address_device(&mut self, slot_id: u8, input: usize, cycle: bool) {
        self.set(
            input as u64,
            0,
            ((slot_id as u32) << 24) |
            ((TrbType::AddressDevice as u32) << 10) |
            (cycle as u32)
        );
    }
    // Synchronizes the input context endpoints with the device context endpoints, it think.
    pub fn configure_endpoint(&mut self, slot_id: u8, input_ctx_ptr: usize, cycle: bool) {
        assert_eq!(input_ctx_ptr & 0xFFFF_FFFF_FFFF_FFF0, input_ctx_ptr);

        self.set(
            (input_ctx_ptr >> 4) as u64,
            0,
            (u32::from(slot_id) << 24) |
            ((TrbType::ConfigureEndpoint as u32) << 10) |
            (cycle as u32),
        )
    }

    pub fn setup(&mut self, setup: usb::Setup, transfer: TransferKind, cycle: bool) {
        self.set(
            unsafe { mem::transmute(setup) },
            8,
            ((transfer as u32) << 16) |
            ((TrbType::SetupStage as u32) << 10) |
            (1 << 6) |
            (cycle as u32)
        );
    }

    pub fn data(&mut self, buffer: usize, length: u16, input: bool, cycle: bool) {
        self.set(
            buffer as u64,
            length as u32,
            ((input as u32) << 16) |
            ((TrbType::DataStage as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn status(&mut self, input: bool, cycle: bool) {
        self.set(
            0,
            0,
            ((input as u32) << 16) |
            ((TrbType::StatusStage as u32) << 10) |
            (1 << 5) |
            (cycle as u32)
        );
    }
}

impl fmt::Debug for Trb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Trb {{ data: {:>016X}, status: {:>08X}, control: {:>08X} }}",
                  self.data.read(), self.status.read(), self.control.read())
    }
}

impl fmt::Display for Trb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({:>016X}, {:>08X}, {:>08X})",
                  self.data.read(), self.status.read(), self.control.read())
    }
}
