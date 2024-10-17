//! The Completion Queue Reactor. Functions like any other async/await reactor, but is driven by
//! IRQs triggering wakeups in order to poll NVME completion queues (see `CompletionFuture`).
//!
//! While the reactor is primarily intended to wait for IRQs and then poll completion queues, it
//! can also be used for notifying when a full submission queue can submit a new command (see
//! `AvailableSqEntryFuture`).

use std::convert::TryFrom;
use std::fs::File;
use std::future::Future;
use std::io::prelude::*;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::{mem, task, thread};

use syscall::data::Event;
use syscall::Result;

use crossbeam_channel::Receiver;

use super::{CmdId, CqId, InterruptSources, Nvme, NvmeCmd, NvmeComp, SqId};

/// A notification request, sent by the future in order to tell the completion thread that the
/// current task wants a notification when a matching completion queue entry has been seen.
#[derive(Debug)]
pub enum NotifReq {
    RequestCompletion {
        cq_id: CqId,
        sq_id: SqId,
        cmd_id: CmdId,

        waker: task::Waker,

        // TODO: Get rid of this allocation, or maybe a thread-local vec for reusing.
        // TODO: Maybe the `remem` crate.
        message: Arc<Mutex<Option<CompletionMessage>>>,
    },
    RequestAvailSubmission {
        sq_id: SqId,
        waker: task::Waker,
    },
}

enum PendingReq {
    PendingCompletion {
        waker: task::Waker,
        message: Arc<Mutex<Option<CompletionMessage>>>,
        cq_id: CqId,
        sq_id: SqId,
        cmd_id: CmdId,
    },
    PendingAvailSubmission {
        waker: task::Waker,
        sq_id: SqId,
    },
}
struct CqReactor {
    int_sources: InterruptSources,
    nvme: Arc<Nvme>,
    pending_reqs: Vec<PendingReq>,
    // used to store commands that may be completed before a completion is requested
    receiver: Receiver<NotifReq>,
    event_queue: File,
}
impl CqReactor {
    fn create_event_queue(int_sources: &mut InterruptSources) -> Result<File> {
        use libredox::flag::*;
        let fd = libredox::call::open("/scheme/event", O_CLOEXEC | O_RDWR, 0)?;
        let mut file = unsafe { File::from_raw_fd(fd as RawFd) };

        for (num, irq_handle) in int_sources.iter_mut() {
            if file
                .write(&Event {
                    id: irq_handle.as_raw_fd() as usize,
                    flags: syscall::EVENT_READ,
                    data: num as usize,
                })
                .unwrap()
                == 0
            {
                panic!("Failed to setup event queue for {} {:?}", num, irq_handle);
            }
        }
        Ok(file)
    }
    fn new(
        nvme: Arc<Nvme>,
        mut int_sources: InterruptSources,
        receiver: Receiver<NotifReq>,
    ) -> Result<Self> {
        Ok(Self {
            event_queue: Self::create_event_queue(&mut int_sources)?,
            int_sources,
            nvme,
            pending_reqs: Vec::new(),
            receiver,
        })
    }
    fn handle_notif_reqs_raw(
        pending_reqs: &mut Vec<PendingReq>,
        receiver: &Receiver<NotifReq>,
        block_until_first: bool,
    ) {
        let mut blocking_iter;
        let mut nonblocking_iter;

        let iter: &mut dyn Iterator<Item = NotifReq> = if block_until_first {
            blocking_iter = std::iter::once(receiver.recv().unwrap()).chain(receiver.try_iter());
            &mut blocking_iter
        } else {
            nonblocking_iter = receiver.try_iter();
            &mut nonblocking_iter
        };

        for req in iter {
            log::trace!("Got notif req: {:?}", req);
            match req {
                NotifReq::RequestCompletion {
                    sq_id,
                    cq_id,
                    cmd_id,
                    waker,
                    message,
                } => pending_reqs.push(PendingReq::PendingCompletion {
                    sq_id,
                    cq_id,
                    cmd_id,
                    message,
                    waker,
                }),
                NotifReq::RequestAvailSubmission { sq_id, waker } => {
                    pending_reqs.push(PendingReq::PendingAvailSubmission { sq_id, waker })
                }
            }
        }
    }
    fn poll_completion_queues(&mut self, iv: u16) -> Option<()> {
        let ivs_read_guard = self.nvme.cqs_for_ivs.read().unwrap();
        let cqs_read_guard = self.nvme.completion_queues.read().unwrap();

        let mut entry_count = 0;

        let cq_ids = ivs_read_guard.get(&iv)?;

        for cq_id in cq_ids.iter().copied() {
            let mut completion_queue_guard = cqs_read_guard.get(&cq_id)?.lock().unwrap();
            let &mut (ref mut completion_queue, _) = &mut *completion_queue_guard;

            while let Some((head, entry)) = completion_queue.complete(None) {
                unsafe { self.nvme.completion_queue_head(cq_id, head) };

                log::trace!(
                    "Got completion queue entry (CQID {}): {:?} at {}",
                    cq_id,
                    entry,
                    head
                );

                {
                    let submission_queues_read_lock = self.nvme.submission_queues.read().unwrap();
                    // this lock is actually important, since it will block during submission from other
                    // threads. the lock won't be held for long by the submitters, but it still prevents
                    // the entry being lost before this reactor is actually able to respond:
                    let &(ref sq_lock, corresponding_cq_id) =
                        submission_queues_read_lock.get(&{ entry.sq_id }).expect(
                            "nvmed: internal error: queue returned from controller doesn't exist",
                        );
                    assert_eq!(cq_id, corresponding_cq_id);
                    let mut sq_guard = sq_lock.lock().unwrap();
                    sq_guard.head = entry.sq_head;
                    // the channel still has to be polled twice though:
                    Self::handle_notif_reqs_raw(&mut self.pending_reqs, &self.receiver, false);
                }

                Self::try_notify_futures(&mut self.pending_reqs, cq_id, &entry);

                entry_count += 1;
            }
        }
        if entry_count == 0 {}

        Some(())
    }
    fn finish_pending_completion(
        pending_reqs: &mut Vec<PendingReq>,
        req_cq_id: CqId,
        cq_id: CqId,
        sq_id: SqId,
        cmd_id: CmdId,
        entry: &NvmeComp,
        i: usize,
    ) -> bool {
        if req_cq_id == cq_id && sq_id == entry.sq_id && cmd_id == entry.cid {
            let (waker, message) = match pending_reqs.remove(i) {
                PendingReq::PendingCompletion { waker, message, .. } => (waker, message),
                _ => unreachable!(),
            };

            *message.lock().unwrap() = Some(CompletionMessage { cq_entry: *entry });
            waker.wake();

            true
        } else {
            false
        }
    }
    fn finish_pending_avail_submission(
        pending_reqs: &mut Vec<PendingReq>,
        sq_id: SqId,
        entry: &NvmeComp,
        i: usize,
    ) -> bool {
        if sq_id == entry.sq_id {
            let waker = match pending_reqs.remove(i) {
                PendingReq::PendingAvailSubmission { waker, .. } => waker,
                _ => unreachable!(),
            };
            waker.wake();

            true
        } else {
            false
        }
    }
    fn try_notify_futures(
        pending_reqs: &mut Vec<PendingReq>,
        cq_id: CqId,
        entry: &NvmeComp,
    ) -> Option<()> {
        let mut i = 0usize;

        let mut futures_notified = 0;

        while i < pending_reqs.len() {
            match &pending_reqs[i] {
                &PendingReq::PendingCompletion {
                    cq_id: req_cq_id,
                    sq_id,
                    cmd_id,
                    ..
                } => {
                    if Self::finish_pending_completion(
                        pending_reqs,
                        req_cq_id,
                        cq_id,
                        sq_id,
                        cmd_id,
                        entry,
                        i,
                    ) {
                        futures_notified += 1;
                    } else {
                        i += 1;
                    }
                }
                &PendingReq::PendingAvailSubmission { sq_id, .. } => {
                    if Self::finish_pending_avail_submission(pending_reqs, sq_id, entry, i) {
                        futures_notified += 1;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        if futures_notified == 0 {}
        Some(())
    }

    fn run(mut self) {
        log::debug!("Running CQ reactor");
        let mut event = Event::default();
        let mut irq_word = [0u8; 8]; // stores the IRQ count

        const WORD_SIZE: usize = mem::size_of::<usize>();

        loop {
            let block_until_first = self.pending_reqs.is_empty();
            Self::handle_notif_reqs_raw(&mut self.pending_reqs, &self.receiver, block_until_first);
            log::trace!("Handled notif reqs");

            // block on getting the next event
            if self.event_queue.read(&mut event).unwrap() == 0 {
                // event queue has been destroyed
                break;
            }

            let (vector, irq_handle) = match self.int_sources.iter_mut().nth(event.data) {
                Some(s) => s,
                None => continue,
            };
            if irq_handle.read(&mut irq_word[..WORD_SIZE]).unwrap() == 0 {
                continue;
            }
            // acknowledge the interrupt (only necessary for level-triggered INTx# interrups)
            if irq_handle.write(&irq_word[..WORD_SIZE]).unwrap() == 0 {
                continue;
            }
            log::trace!("NVME IRQ: vector {}", vector);
            self.nvme.set_vector_masked(vector, true);
            self.poll_completion_queues(vector);
            self.nvme.set_vector_masked(vector, false);
        }
    }
}

pub fn start_cq_reactor_thread(
    nvme: Arc<Nvme>,
    interrupt_sources: InterruptSources,
    receiver: Receiver<NotifReq>,
) -> thread::JoinHandle<()> {
    // Actually, nothing prevents us from spawning additional threads. the channel is MPMC and
    // everything is properly synchronized. I'm not saying this is strictly required, but with
    // multiple completion queues it might actually be worth considering. The (in-kernel) IRQ
    // subsystem can have some room for improvement regarding lowering the latency, but MSI-X allows
    // multiple vectors to point to different CPUs, so that the load can be balanced across the
    // logical processors.
    let reactor = CqReactor::new(nvme, interrupt_sources, receiver)
        .expect("nvmed: failed to setup CQ reactor");
    thread::spawn(move || reactor.run())
}

#[derive(Debug)]
pub struct CompletionMessage {
    cq_entry: NvmeComp,
}

pub enum CompletionFutureState<'a, F> {
    // the future is in its initial state: the command has not been submitted yet, and no interest
    // has been registered. this state will repeat until a free submission queue entry appears to
    // it, which it probably will since queues aren't supposed to be nearly always be full.
    PendingSubmission {
        cmd_init: F,
        nvme: &'a Nvme,
        sq_id: SqId,
    },
    PendingCompletion {
        nvme: &'a Nvme,
        cq_id: CqId,
        cmd_id: CmdId,
        sq_id: SqId,
        message: Arc<Mutex<Option<CompletionMessage>>>,
    },
    Finished,
    Placeholder,
}
pub struct CompletionFuture<'a, F> {
    pub state: CompletionFutureState<'a, F>,
}

// enum not self-referential
impl<F> Unpin for CompletionFuture<'_, F> {}

impl<F> Future for CompletionFuture<'_, F>
where
    F: FnOnce(CmdId) -> NvmeCmd,
{
    type Output = NvmeComp;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        let this = &mut self.get_mut().state;

        match mem::replace(this, CompletionFutureState::Placeholder) {
            CompletionFutureState::PendingSubmission {
                cmd_init,
                nvme,
                sq_id,
            } => {
                let sqs_read_guard = nvme.submission_queues.read().unwrap();
                let &(ref sq_lock, cq_id) = sqs_read_guard
                    .get(&sq_id)
                    .expect("nvmed: internal error: given SQ for SQ ID not there");
                let mut sq_guard = sq_lock.lock().unwrap();
                let sq = &mut *sq_guard;

                if sq.is_full() {
                    // when the CQ reactor gets a new completion queue entry, it'll lock the
                    // submisson queue it came from. since we're holding the same lock, this
                    // message will always be sent before the reactor is done with the entry.
                    nvme.reactor_sender
                        .send(NotifReq::RequestAvailSubmission {
                            sq_id,
                            waker: context.waker().clone(),
                        })
                        .unwrap();
                    *this = CompletionFutureState::PendingSubmission {
                        cmd_init,
                        nvme,
                        sq_id,
                    };
                    return task::Poll::Pending;
                }

                let cmd_id = u16::try_from(sq.tail)
                    .expect("nvmed: internal error: CQ has more than 2^16 entries");
                let tail = sq.submit_unchecked(cmd_init(cmd_id));
                let tail = u16::try_from(tail).unwrap();

                // make sure that we register interest before the reactor can get notified
                let message = Arc::new(Mutex::new(None));
                *this = CompletionFutureState::PendingCompletion {
                    nvme,
                    cq_id,
                    cmd_id,
                    sq_id,
                    message: Arc::clone(&message),
                };
                nvme.reactor_sender
                    .send(NotifReq::RequestCompletion {
                        cq_id,
                        sq_id,
                        cmd_id,
                        message,
                        waker: context.waker().clone(),
                    })
                    .expect("reactor dead");
                unsafe { nvme.submission_queue_tail(sq_id, tail) };
                task::Poll::Pending
            }
            CompletionFutureState::PendingCompletion {
                message,
                cq_id,
                cmd_id,
                sq_id,
                nvme,
            } => {
                if let Some(value) = message.lock().unwrap().take() {
                    *this = CompletionFutureState::Finished;
                    return task::Poll::Ready(value.cq_entry);
                }
                nvme.reactor_sender
                    .send(NotifReq::RequestCompletion {
                        cq_id,
                        sq_id,
                        cmd_id,
                        waker: context.waker().clone(),
                        message: Arc::clone(&message),
                    })
                    .expect("reactor dead");
                *this = CompletionFutureState::PendingCompletion {
                    message,
                    cq_id,
                    cmd_id,
                    sq_id,
                    nvme,
                };
                task::Poll::Pending
            }
            CompletionFutureState::Finished => {
                panic!("calling poll() on an already finished CompletionFuture")
            }
            CompletionFutureState::Placeholder => unreachable!(),
        }
    }
}
