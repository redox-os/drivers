use std::{fmt, mem};
use syscall::io::{Io, Mmio};
use usb;

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
    pub fn reserved(&mut self, cycle: bool) {
        self.data.write(0);
        self.status.write(0);
        self.control.write(
            ((TrbType::Reserved as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn no_op_cmd(&mut self, cycle: bool) {
        self.data.write(0);
        self.status.write(0);
        self.control.write(
            ((TrbType::NoOpCmd as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn enable_slot(&mut self, slot_type: u8, cycle: bool) {
        self.data.write(0);
        self.status.write(0);
        self.control.write(
            (((slot_type as u32) & 0x1F) << 16) |
            ((TrbType::EnableSlot as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn address_device(&mut self, slot_id: u8, input: usize, cycle: bool) {
        self.data.write(input as u64);
        self.status.write(0);
        self.control.write(
            ((slot_id as u32) << 24) |
            ((TrbType::AddressDevice as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn setup(&mut self, setup: usb::Setup, transfer: TransferKind, cycle: bool) {
        self.data.write(unsafe { mem::transmute(setup) });
        self.status.write((0 << 22) | 8);
        self.control.write(
            ((transfer as u32) << 16) |
            ((TrbType::SetupStage as u32) << 10) |
            (1 << 6) |
            (cycle as u32)
        );
    }

    pub fn data(&mut self, buffer: usize, length: u16, input: bool, cycle: bool) {
        self.data.write(buffer as u64);
        self.status.write((0 << 22) | length as u32);
        self.control.write(
            ((input as u32) << 16) |
            ((TrbType::DataStage as u32) << 10) |
            (cycle as u32)
        );
    }

    pub fn status(&mut self, input: bool, cycle: bool) {
        self.data.write(0);
        self.status.write(0 << 22);
        self.control.write(
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
