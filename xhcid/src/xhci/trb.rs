use std::fmt;
use syscall::io::{Io, Mmio};

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

#[repr(packed)]
pub struct Trb {
    pub data: Mmio<u64>,
    pub status: Mmio<u32>,
    pub control: Mmio<u32>,
}

impl Trb {
    pub fn reset(&mut self, param: u64, status: u32, control: u16, trb_type: TrbType, cycle: bool) {
        let full_control =
            (control as u32) << 16 |
            ((trb_type as u32) & 0x3F) << 10 |
            if cycle { 1 << 0 } else { 0 };

        self.data.write(param);
        self.status.write(status);
        self.control.write(full_control);
    }

    pub fn reserved(&mut self, cycle: bool) {
        self.reset(0, 0, 0, TrbType::Reserved, cycle);
    }

    pub fn no_op_cmd(&mut self, cycle: bool) {
        self.reset(0, 0, 0, TrbType::NoOpCmd, cycle);
    }

    pub fn enable_slot(&mut self, slot_type: u8, cycle: bool) {
        self.reset(0, 0, (slot_type as u16) & 0x1F, TrbType::EnableSlot, cycle);
    }

    pub fn address_device(&mut self, slot_id: u8, input: usize, cycle: bool) {
        self.reset(input as u64, 0, (slot_id as u16) << 8, TrbType::AddressDevice, cycle);
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
