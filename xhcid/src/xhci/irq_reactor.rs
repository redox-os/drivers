use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{self, AtomicUsize};
use std::{mem, task, thread};

use crossbeam_channel::{Sender, Receiver};
use futures::Stream;

use super::Xhci;
use super::ring::Ring;
use super::trb::{Trb, TrbCompletionCode, TrbType};

/// Short-term states (as in, they are removed when the waker is consumed, but probably pushed back
/// by the future unless it completed).
pub struct State {
    waker: task::Waker,
    kind: StateKind,
    message: Arc<Mutex<Option<NextEventTrb>>>,
    is_isoch_or_vf: bool,
}

pub struct NextEventTrb {
    pub event_trb: Trb,
    pub src_trb: Option<Trb>,
}

// TODO: Perhaps all of the transfer rings used by the xHC should be stored linearly, and then
// indexed using this struct instead.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RingId {
    pub slot: u8,
    pub endpoint_num: u8,
    pub stream_id: u16,
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
    Transfer { phys_ptr: u64, ring_id: RingId },
    Other(TrbType),
}

impl StateKind {
    pub fn trb_type(&self) -> TrbType {
        match self.kind {
            Self::CommandCompletion { .. } => TrbType::CommandCompletion,
            Self::Transfer { .. } => TrbType::Transfer,
            Self::Other(ty) => ty,
        }
    }
}


pub struct IrqReactor {
    hci: Arc<Xhci>,
    current_count: Arc<AtomicUsize>,
    irq_file: Option<File>,
    receiver: Receiver<NewPendingTrb>,

    states: Vec<State>,

    // TODO: Since the IRQ reactor is the only part of this driver that gets event TRBs, perhaps
    // the event ring should be owned here?
}

pub type NewPendingTrb = State;

pub fn start_irq_reactor(hci: Arc<Xhci>, irq_file: Option<File>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        IrqReactor::new(hci, irq_file).run()
    })
}

impl IrqReactor {
    pub fn new(hci: Arc<Xhci>, irq_file: Option<File>) -> Self {
        Self {
            hci,
            irq_file,
            current_count: Arc::new(AtomicUsize::new()),
        }
    }
    // TODO: Configure the amount of time to be awaited when no more work can be done.
    fn pause(&self) {
        std::thread::yield_now();
    }
    fn run_polling(mut self) {
        loop {
            self.handle_requests();

            let index = self.hci.primary_event_ring.lock().unwrap().next_index();

            let mut trb;

            'busy_waiting: loop {
                trb = self.hci.primary_event_ring.lock().unwrap().trbs[index];

                if trb.completion_code() == TrbCompletionCode::Invalid as u8 {
                    self.pause();
                    continue 'busy_waiting;
                }
            }
            self.acknowledge(trb);
            self.update_erdp();
        }
    }
    fn run_with_irq_file(mut self) {
        'event_loop: loop {
            self.handle_requests();

            let irq_file = self.irq_file.as_mut().unwrap();

            let mut buffer = [0u8; 8];
            let bytes_read = self.irq_file.read(&mut buffer).expect("Failed to read from irq scheme");
            if bytes_read < mem::size_of::<usize>() {
                panic!("wrong number of bytes read from `irq:`: expected {}, got {}", mem::size_of::<usize>(), bytes_read);
            }

            if !self.hci.received_irq() {
                continue;
            }

            let _ = self.irq_file.write(&buffer);

            // TODO: More event rings, probably even with different IRQs.

            let event_ring = self.hci.primary_event_ring.lock().unwrap();
            let trb = event_ring.next();

            if trb.completion_code() == TrbCompletionCode::Invalid as u8 {
                println!("xhci: Received interrupt, but no event was found in the event ring. Ignoring interrupt.");
                continue 'event_loop;
            }

            self.acknowledge(*trb);
            trb.reserved(false);

            self.update_erdp();
        }
    }
    fn update_erdp(&self) {
        let dequeue_pointer_and_dcs = self.hci.primary_event_ring.lock().unwrap().register();
        let dequeue_pointer = dequeue_pointer_and_dcs & 0xFFFF_FFFF_FFFF_FFFE;
        assert_eq!(dequeue_pointer & 0xFFFF_FFFF_FFFF_FFF0, dequeue_pointer, "unaligned ERDP received from primary event ring");

        self.xhci.run.lock().unwrap().ints[0].erdp.write(dequeue_pointer);
    }
    fn handle_requests(&mut self) {
        self.states.extend(self.receiver.try_iter());
    }
    fn acknowledge(&mut self, trb: Trb) {
        let mut index = 0;

        loop {
            match self.states[index].kind {
                StateKind::CommandCompletion { phys_ptr } if trb.trb_type() == TrbType::CommandCompletion as u8 => if trb.completion_trb_pointer() == Some(phys_ptr) {
                    let state = self.states.remove(index).unwrap();

                    // Before waking, it's crucial that the command TRB that generated this event
                    // be fetched before removing this event TRB from the queue.
                    let command_trb = match self.hci.command_ring.lock().unwrap().ring.phys_addr_to_entry_mut(phys_ptr) {
                        Some(command_trb) => {
                            let t = command_trb.clone();
                            command_trb.reserved(false);
                            t
                        },
                        None => {
                            println!("The xHC supplied a pointer to a command TRB that was outside the known command ring bounds. Ignoring event TRB {:?}.", trb);
                            continue;
                        }
                    };

                    // TODO: Validate the command TRB.
                    *state.message.lock().unwrap() = Some(NextEventTrb {
                        src_trb: command_trb.clone(),
                        event_trb: trb,
                    });

                    state.waker.wake();
                } else if trb.completion_trb_pointer().is_none() {
                    println!("Command TRB somehow resulted in an error that only can be caused by transfer TRBs. Ignoring event TRB: {:?}.", trb);
                    continue;
                } else {
                    // The event TRB simply didn't match the current future
                    continue;
                }

                StateKind::Transfer { phys_ptr, ring_id } if trb.trb_type() == TrbType::Transfer as u8 => if let Some(src_trb) = self.xhc.lock().unwrap().get_transfer_trb(trb.transfer_event_trb_pointer(), ring_id) {
                    if trb.transfer_event_trb_pointer() == Some(phys_ptr) {
                        // Give the source transfer TRB together with the event TRB, to the future.

                        let state = self.states.remove(index).unwrap();
                        *state.message.lock().unwrap() = Some(NextEventTrb {
                            src_trb,
                            event_trb: trb,
                        });
                        state.waker.wake();
                    } else if trb.transfer_event_trb_pointer().is_none() {
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
                    } else {
                        // The event TRB simply didn't match the current future
                        continue;
                    }
                } else { continue }

                StateKind::Other(trb_type) if trb_type as u8 == trb.trb_type() => {
                    let state = self.states.remove(index).unwrap();
                    state.waker.wake();
                }

                _ => {
                    index += 1;
                    if index >= self.states.len() {
                        break;
                    }
                    continue;
                }
            }
        }
    }
    pub fn acknowledge_failed_transfer_trbs(&mut self, trb: Trb) {
        let mut index = 0;

        loop {
            if ! self.states[index].is_isoch_or_vf {
                index += 1;
                if index >= self.states.len() {
                    break;
                }
                continue;
            }
            let state = self.states.remove(index).unwrap();
            *state.message.lock().unwrap() = Some(NextEventTrb {
                event_trb: trb,
                src_trb: None,
            });
            state.waker.wake();
        }
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

enum EventTrbFuture {
    Pending { state: FutureState, sender: Sender<State>, },
    Finished,
}

impl Future for EventTrbFuture {
    type Output = NextEventTrb;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        match self.get_mut() {
            &mut Self::Pending { ref mut state, ref mut sender } => if let Some(message) = state.message.lock().unwrap().take() {
                *self.get_mut() = Self::Finished;

                task::Poll::Ready(message)
            } else {
                sender.send(State {
                    message: Arc::clone(&state.message),
                    is_isoch_or_vf: state.is_isoch_or_vf,
                    state_kind: state.state_kind,
                    waker: context.waker().clone(),
                }).expect("IRQ reactor thread unexpectedly stopped");

                task::Poll::Pending
            }
            &mut Self::Finished => panic!("Polling finished EventTrbFuture again."),
        }
    }
}

impl Xhci {
    pub fn get_transfer_trb(&self, paddr: u64, id: RingId) -> Option<Trb> {
        self.with_ring(id, |ring| ring.phys_addr_to_entry(paddr))
    }
    pub fn with_ring<T, F: FnOnce(&Ring) -> T>(&self, id: RingId, function: F) -> T {
        use super::RingOrStreams;

        let slot_state = self.slot_states.get(&id.slot)?;
        let endpoint_state = slot_state.endpoint_states.get(&id.endpoint_num)?;

        let ring_ref = match endpoint_state.transfer {
            RingOrStreams::Ring(ref ring) => ring,
            RingOrStreams::Streams(ref ctx_arr) => ctx_arr.rings.get(&id.stream_id)?,
        };

        function(ring_ref)
    }
    pub fn with_ring_mut<T, F: FnOnce(&mut Ring) -> T>(&self, id: RingId, function: F) -> T {
        use super::RingOrStreams;

        let slot_state = self.slot_states.get(&id.slot)?;
        let endpoint_state = slot_state.endpoint_states.get_mut(&id.endpoint_num)?;

        let ring_ref = match endpoint_state.transfer {
            RingOrStreams::Ring(ref mut ring) => ring,
            RingOrStreams::Streams(ref mut ctx_arr) => ctx_arr.rings.get_mut(&id.stream_id)?,
        };

        function(ring_ref)
    }
    pub fn next_transfer_event_trb(&self, ring_id: RingId, trb: &Trb) -> impl Future<Output = NextEventTrb> + Send + Sync + 'static {
        if ! trb.is_transfer_trb() {
            panic!("Invalid TRB type given to next_transfer_event_trb(): {} (TRB {:?}. Expected transfer TRB.", trb.trb_type(), trb)
        }

        let is_isoch_or_vf = trb.trb_type() == TrbType::Isoch as u8;

        EventTrbFuture::Pending {
            state: FutureState {
                is_isoch_or_vf,
                state_kind: StateKind::Transfer {
                    ring_id,
                    phys_ptr: self.with_ring(ring_id, |ring| ring.trb_phys_ptr(trb)/*.expect("Invalid TRB: transfer TRB wasn't in the ring specified. Only direct references to the TRBs of a ring can be used (ring address range: {:p}-{:p}).", ring.start_addr(), ring.end_addr())*/),
                },
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
        }
    }
    pub fn next_command_completion_event_trb(&self, trb: &Trb) -> impl Future<Output = NextEventTrb> + Send + Sync + 'static {
        if ! trb.is_command_trb() {
            panic!("Invalid TRB type given to next_command_completion_event_trb(): {} (TRB {:?}. Expected command TRB.", trb.trb_type(), trb)
        }

        let command_ring = self.cmd.lock().unwrap();

        EventTrbFuture::Pending {
            state: FutureState {
                // This is only possible for transfers if they are isochronous, or for Force Event TRBs (virtualization).
                is_isoch_or_vf: false,
                state_kind: StateKind::CommandCompletion {
                    phys_ptr: command_ring.trb_phys_ptr(trb),//.expect("Invalid TRB: expected a command TRB within the address range of the command TRB ({:p} {:p}), found TRB {:?} at {:p}", ring.start_addr(), ring.end_addr(), trb, trb)
                },
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
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
        if ! valid_trb_types.contains(&trb_type) {
            panic!("Invalid TRB type given to next_misc_event_trb(): {:?}. Only event TRB types that are neither transfer events or command completion events can be used.", trb_type)
        }
        EventTrbFuture::Pending {
            state: FutureState {
                is_isoch_or_vf: false,
                state_kind: StateKind::Other(trb_type),
                message: Arc::new(Mutex::new(None)),
            },
            sender: self.irq_reactor_sender.clone(),
        }
    }
}
