use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::prelude::*;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{self, AtomicUsize};
use std::{io, mem, task, thread};

use std::os::unix::io::AsRawFd;

use crossbeam_channel::{Sender, Receiver};
use log::{debug, error, info, warn, trace};
use futures::Stream;
use syscall::Io;

use event::{Event, EventQueue, RawEventQueue};

use super::Xhci;
use super::doorbell::Doorbell;
use super::ring::Ring;
use super::trb::{Trb, TrbCompletionCode, TrbType};
use super::event::EventRing;

/// Short-term states (as in, they are removed when the waker is consumed, but probably pushed back
/// by the future unless it completed).
#[derive(Debug)]
pub struct State {
    waker: task::Waker,
    kind: StateKind,
    message: Arc<Mutex<Option<NextEventTrb>>>,
    is_isoch_or_vf: bool,
}

#[derive(Debug)]
pub struct NextEventTrb {
    pub event_trb: Trb,
    pub src_trb: Option<Trb>,
}

// TODO: Perhaps all of the transfer rings used by the xHC should be stored linearly, and then
// indexed using this struct instead.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RingId {
    pub port: u8,
    pub endpoint_num: u8,
    pub stream_id: u16,
}
impl RingId {
    pub const fn default_control_pipe(port: u8) -> Self {
        Self {
            port,
            endpoint_num: 0,
            stream_id: 0,
        }
    }
}

/// The state specific to a TRB-type. Since some of the event TDs may asynchronously appear, for
/// example the Command Completion Event and the Transfer Event TDs, they have to be
/// distinguishable. Luckily, the xHC also gives us the actual (physical) pointer to the source
/// TRB, from the command ring, unless the event TD has one the completion codes Ring Underrun,
/// Ring Overrun, or VF Event Ring Full Error. When these errors are encountered, it simply
/// indicates that the commands causing the errors continue to be pending, and thus no information
/// is lost.
#[derive(Clone, Copy, Debug)]
pub enum StateKind {
    CommandCompletion { phys_ptr: u64 },
    Transfer { first_phys_ptr: u64, last_phys_ptr: u64, ring_id: RingId },
    Other(TrbType),
}

impl StateKind {
    pub fn trb_type(&self) -> TrbType {
        match self {
            &Self::CommandCompletion { .. } => TrbType::CommandCompletion,
            &Self::Transfer { .. } => TrbType::Transfer,
            &Self::Other(ty) => ty,
        }
    }
}


pub struct IrqReactor {
    hci: Arc<Xhci>,
    irq_file: Option<File>,
    receiver: Receiver<NewPendingTrb>,

    states: Vec<State>,

    // TODO: Since the IRQ reactor is the only part of this driver that gets event TRBs, perhaps
    // the event ring should be owned here?
}

pub type NewPendingTrb = State;

impl IrqReactor {
    pub fn new(hci: Arc<Xhci>, receiver: Receiver<NewPendingTrb>, irq_file: Option<File>) -> Self {
        Self {
            hci,
            irq_file,
            receiver,
            states: Vec::new(),
        }
    }
    // TODO: Configure the amount of time wait when no more work can be done (for IRQ-less polling).
    fn pause(&self) {
        std::thread::yield_now();
    }
    fn run_polling(mut self) {
        debug!("Running IRQ reactor in polling mode.");
        let hci_clone = Arc::clone(&self.hci);

        let mut event_trb_index = { hci_clone.primary_event_ring.lock().unwrap().ring.next_index() };

        'trb_loop: loop {
            self.pause();

            let mut event_ring = hci_clone.primary_event_ring.lock().unwrap();

            let event_trb = &mut event_ring.ring.trbs[event_trb_index];

            if event_trb.completion_code() == TrbCompletionCode::Invalid as u8 {
                continue 'trb_loop;
            }

            trace!("Found event TRB: {:?}", event_trb);

            if self.check_event_ring_full(event_trb.clone()) {
                info!("Had to resize event TRB, retrying...");
                hci_clone.event_handler_finished();
                continue 'trb_loop;
            }

            self.handle_requests();
            self.acknowledge(event_trb.clone());

            event_trb.reserved(false);

            self.update_erdp(&*event_ring);

            event_trb_index = event_ring.ring.next_index();
        }
    }
    fn run_with_irq_file(mut self) {
        debug!("Running IRQ reactor with IRQ file and event queue");

        let hci_clone = Arc::clone(&self.hci);
        let mut event_queue = RawEventQueue::new().expect("xhcid irq_reactor: failed to create IRQ event queue");
        let irq_fd = self.irq_file.as_ref().unwrap().as_raw_fd();
        event_queue.subscribe(irq_fd as usize, 0, event::EventFlags::READ).unwrap();

        let mut event_trb_index = { hci_clone.primary_event_ring.lock().unwrap().ring.next_index() };

        for _event in event_queue {
            trace!("IRQ event queue notified");
            let mut buffer = [0u8; 8];

            let _ = self.irq_file.as_mut().unwrap().read(&mut buffer).expect("Failed to read from irq scheme");

            if !self.hci.received_irq() {
                // continue only when an IRQ to this device was received
                trace!("no interrupt pending");
                break;
            }

            trace!("IRQ reactor received an IRQ");

            let _ = self.irq_file.as_mut().unwrap().write(&buffer);

            // TODO: More event rings, probably even with different IRQs.

            let mut event_ring = hci_clone.primary_event_ring.lock().unwrap();

            let mut count = 0;

            loop {
                let event_trb = &mut event_ring.ring.trbs[event_trb_index];

                if event_trb.completion_code() == TrbCompletionCode::Invalid as u8 {
                    if count == 0 { warn!("xhci: Received interrupt, but no event was found in the event ring. Ignoring interrupt.") }
                    // no more events were found, continue the loop
                    return;
                } else { count += 1 }

                trace!("Found event TRB type {}: {:?}", event_trb.trb_type(), event_trb);

                if self.check_event_ring_full(event_trb.clone()) {
                    info!("Had to resize event TRB, retrying...");
                    hci_clone.event_handler_finished();
                    return;
                }

                self.handle_requests();
                self.acknowledge(event_trb.clone());

                event_trb.reserved(false);

                self.update_erdp(&*event_ring);

                event_trb_index = event_ring.ring.next_index();
            }
        }
    }
    fn update_erdp(&self, event_ring: &EventRing) {
        let dequeue_pointer_and_dcs = event_ring.erdp();
        let dequeue_pointer = dequeue_pointer_and_dcs & 0xFFFF_FFFF_FFFF_FFFE;
        assert_eq!(dequeue_pointer & 0xFFFF_FFFF_FFFF_FFF0, dequeue_pointer, "unaligned ERDP received from primary event ring");

        trace!("Updated ERDP to {:#0x}", dequeue_pointer);

        self.hci.run.lock().unwrap().ints[0].erdp_low.write(dequeue_pointer as u32);
        self.hci.run.lock().unwrap().ints[0].erdp_high.write((dequeue_pointer >> 32) as u32);
    }
    fn handle_requests(&mut self) {
        self.states.extend(self.receiver.try_iter().inspect(|req| trace!("Received request: {:X?}", req)));
    }
    fn acknowledge(&mut self, trb: Trb) {
        //TODO: handle TRBs without an attached state

        trace!("ACK TRB {:X?}", trb);

        let mut index = 0;
        while index < self.states.len() {
            trace!("ACK STATE {}: {:X?}", index, self.states[index].kind);

            match self.states[index].kind {
                StateKind::CommandCompletion { phys_ptr } if trb.trb_type() == TrbType::CommandCompletion as u8 => {
                    if trb.completion_trb_pointer() == Some(phys_ptr) {
                        trace!("Found matching command completion future");
                        let state = self.states.remove(index);

                        // Before waking, it's crucial that the command TRB that generated this event
                        // is fetched before removing this event TRB from the queue.
                        let command_trb = match self.hci.cmd.lock().unwrap().phys_addr_to_entry_mut(self.hci.cap.ac64(), phys_ptr) {
                            Some(command_trb) => {
                                let t = command_trb.clone();
                                command_trb.reserved(false);
                                t
                            },
                            None => {
                                warn!("The xHC supplied a pointer to a command TRB that was outside the known command ring bounds. Ignoring event TRB {:?}.", trb);
                                continue;
                            }
                        };

                        // TODO: Validate the command TRB.
                        *state.message.lock().unwrap() = Some(NextEventTrb {
                            src_trb: Some(command_trb.clone()),
                            event_trb: trb.clone(),
                        });

                        trace!("Waking up future with waker: {:?}", state.waker);
                        state.waker.wake();

                        return;
                    } else if trb.completion_trb_pointer().is_none() {
                        warn!("Command TRB somehow resulted in an error that only can be caused by transfer TRBs. Ignoring event TRB: {:?}.", trb);
                    }
                }

                StateKind::Transfer { first_phys_ptr, last_phys_ptr, ring_id } if trb.trb_type() == TrbType::Transfer as u8 => {
                    if let Some(src_trb) = trb.transfer_event_trb_pointer().map(|ptr| self.hci.get_transfer_trb(ptr, ring_id)).flatten() {
                        match trb.transfer_event_trb_pointer() {
                            Some(phys_ptr) => {
                                let matches = if first_phys_ptr <= last_phys_ptr {
                                    phys_ptr >= first_phys_ptr && phys_ptr <= last_phys_ptr
                                } else {
                                    // Handle ring buffer wrap
                                    phys_ptr >= first_phys_ptr || phys_ptr <= last_phys_ptr
                                };
                                if matches {
                                    // Give the source transfer TRB together with the event TRB, to the future.
                                    let state = self.states.remove(index);
                                    *state.message.lock().unwrap() = Some(NextEventTrb {
                                        src_trb: Some(src_trb),
                                        event_trb: trb.clone(),
                                    });
                                    state.waker.wake();
                                    return;
                                }
                            },
                            None => {
                                // Ring Overrun, Ring Underrun, or Virtual Function Event Ring Full.
                                //
                                // These errors are caused when either an isoch transfer that shall write data, doesn't
                                // have any data since the ring is empty, or if an isoch receive is impossible due to a
                                // full ring. The Virtual Function Event Ring Full is only for Virtual Machine
                                // Managers, and since this isn't implemented yet, they are irrelevant.
                                //
                                // The best solution here is to differentiate between isoch transfers (and
                                // virtual function event rings when virtualization gets implemented), with
                                // regular commands and transfers, and send the error TRB to all of them, or
                                // possibly an error code wrapped in a Result.
                                self.acknowledge_failed_transfer_trbs(trb);
                                return;
                            }
                        }
                    }
                }

                StateKind::Other(trb_type) if trb_type as u8 == trb.trb_type() => {
                    let state = self.states.remove(index);
                    state.waker.wake();
                    return;
                }

                _ => ()
            }

            index += 1;
        }
        warn!("Lost event TRB type {}, completion code: {}: {:X?}", trb.trb_type(), trb.completion_code(), trb);
    }
    fn acknowledge_failed_transfer_trbs(&mut self, trb: Trb) {
        let mut index = 0;

        loop {
            if ! self.states[index].is_isoch_or_vf {
                index += 1;
                if index >= self.states.len() {
                    break;
                }
                continue;
            }
            let state = self.states.remove(index);
            *state.message.lock().unwrap() = Some(NextEventTrb {
                event_trb: trb.clone(),
                src_trb: None,
            });
            state.waker.wake();
        }
    }
    /// Checks if an event TRB is a Host Controller Event, with the completion code Event Ring
    /// Full. If so, it grows the event ring. The return value is whether the event ring was full,
    /// and then grown.
    fn check_event_ring_full(&mut self, event_trb: Trb) -> bool {
        let had_event_ring_full_error =  event_trb.trb_type() == TrbType::HostController as u8 && event_trb.completion_code() == TrbCompletionCode::EventRingFull as u8;

        if had_event_ring_full_error {
            self.grow_event_ring();
        }
        had_event_ring_full_error
    }
    /// Grows the event ring
    fn grow_event_ring(&mut self) {
        // TODO
        error!("TODO: grow event ring");
    }

    pub fn run(mut self) {
        if self.irq_file.is_some() {
            self.run_with_irq_file();
        } else {
            self.run_polling();
        }
    }
}

struct FutureState {
    message: Arc<Mutex<Option<NextEventTrb>>>,
    is_isoch_or_vf: bool,
    state_kind: StateKind,
}

pub struct EventDoorbell {
    dbs: Arc<Mutex<&'static mut [Doorbell]>>,
    index: usize,
    data: u32,
}

impl EventDoorbell {
    pub fn new(hci: &Xhci, index: usize, data: u32) -> Self {
        Self {
            //TODO: simplify this logic, maybe just use a raw pointer?
            dbs: hci.dbs.clone(),
            index,
            data,
        }
    }

    pub fn ring(self) {
        trace!("Ring doorbell {} with data {}", self.index, self.data);
        self.dbs.lock().unwrap()[self.index].write(self.data);
    }
}

enum EventTrbFuture {
    Pending { state: FutureState, sender: Sender<State>, doorbell_opt: Option<EventDoorbell> },
    Finished,
}

impl Future for EventTrbFuture {
    type Output = NextEventTrb;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        let this = self.get_mut();

        let message = match this {
            &mut Self::Pending { ref state, ref sender, ref mut doorbell_opt } => match state.message.lock().unwrap().take() {
                Some(message) => message,

                None => {
                    // Register state with IRQ reactor
                    trace!("Send state {:X?}", state.state_kind);
                    sender.send(State {
                        message: Arc::clone(&state.message),
                        is_isoch_or_vf: state.is_isoch_or_vf,
                        kind: state.state_kind,
                        waker: context.waker().clone(),
                    }).expect("IRQ reactor thread unexpectedly stopped");

                    // Doorbell must be rung after sending state
                    if let Some(doorbell) = doorbell_opt.take() {
                        doorbell.ring();
                    }

                    return task::Poll::Pending;
                }
            }
            &mut Self::Finished => panic!("Polling finished EventTrbFuture again."),
        };
        *this = Self::Finished;
        task::Poll::Ready(message)
    }
}

impl Xhci {
    pub fn get_transfer_trb(&self, paddr: u64, id: RingId) -> Option<Trb> {
        self.with_ring(id, |ring| ring.phys_addr_to_entry(self.cap.ac64(), paddr)).flatten()
    }
    pub fn with_ring<T, F: FnOnce(&Ring) -> T>(&self, id: RingId, function: F) -> Option<T> {
        use super::RingOrStreams;

        let slot_state = self.port_states.get(&(id.port as usize))?;
        let endpoint_state = slot_state.endpoint_states.get(&id.endpoint_num)?;

        let ring_ref = match endpoint_state.transfer {
            RingOrStreams::Ring(ref ring) => ring,
            RingOrStreams::Streams(ref ctx_arr) => ctx_arr.rings.get(&id.stream_id)?,
        };

        Some(function(ring_ref))
    }
    pub fn with_ring_mut<T, F: FnOnce(&mut Ring) -> T>(&self, id: RingId, function: F) -> Option<T> {
        use super::RingOrStreams;

        let mut slot_state = self.port_states.get_mut(&(id.port as usize))?;
        let mut endpoint_state = slot_state.endpoint_states.get_mut(&id.endpoint_num)?;

        let ring_ref = match endpoint_state.transfer {
            RingOrStreams::Ring(ref mut ring) => ring,
            RingOrStreams::Streams(ref mut ctx_arr) => ctx_arr.rings.get_mut(&id.stream_id)?,
        };

        Some(function(ring_ref))
    }
    pub fn next_transfer_event_trb(&self, ring_id: RingId, ring: &Ring, first_trb: &Trb, last_trb: &Trb, doorbell: EventDoorbell) -> impl Future<Output = NextEventTrb> + Send + Sync + 'static {
        if ! last_trb.is_transfer_trb() {
            panic!("Invalid TRB type given to next_transfer_event_trb(): {} (TRB {:?}. Expected transfer TRB.", last_trb.trb_type(), last_trb)
        }

        let is_isoch_or_vf = last_trb.trb_type() == TrbType::Isoch as u8;
        let first_phys_ptr = ring.trb_phys_ptr(self.cap.ac64(), first_trb);
        let last_phys_ptr = ring.trb_phys_ptr(self.cap.ac64(), last_trb);
        EventTrbFuture::Pending {
            state: FutureState {
                is_isoch_or_vf,
                state_kind: StateKind::Transfer {
                    ring_id,
                    first_phys_ptr,
                    last_phys_ptr,
                },
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
            doorbell_opt: Some(doorbell),
        }
    }
    pub fn next_command_completion_event_trb(&self, command_ring: &Ring, trb: &Trb, doorbell: EventDoorbell) -> impl Future<Output = NextEventTrb> + Send + Sync + 'static {
        if ! trb.is_command_trb() {
            panic!("Invalid TRB type given to next_command_completion_event_trb(): {} (TRB {:?}. Expected command TRB.", trb.trb_type(), trb)
        }
        EventTrbFuture::Pending {
            state: FutureState {
                // This is only possible for transfers if they are isochronous, or for Force Event TRBs (virtualization).
                is_isoch_or_vf: false,
                state_kind: StateKind::CommandCompletion {
                    phys_ptr: command_ring.trb_phys_ptr(self.cap.ac64(), trb),
                },
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
            doorbell_opt: Some(doorbell),
        }
    }
    pub fn next_misc_event_trb(&self, trb_type: TrbType) -> impl Future<Output = NextEventTrb> + Send + Sync + 'static {
        let valid_trb_types = [
            TrbType::PortStatusChange as u8,
            TrbType::BandwidthRequest as u8,
            TrbType::Doorbell as u8,
            TrbType::HostController as u8,
            TrbType::DeviceNotification as u8,
            TrbType::MfindexWrap as u8,
        ];
        if ! valid_trb_types.contains(&(trb_type as u8)) {
            panic!("Invalid TRB type given to next_misc_event_trb(): {:?}. Only event TRB types that are neither transfer events or command completion events can be used.", trb_type)
        }
        EventTrbFuture::Pending {
            state: FutureState {
                is_isoch_or_vf: false,
                state_kind: StateKind::Other(trb_type),
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
            doorbell_opt: None,
        }
    }

}
