//! The Completion Queue Reactor. Functions like any other async/await reactor, but is driven by
//! IRQs triggering wakeups in order to poll NVME completion queues (see `CompletionFuture`).
//!
//! While the reactor is primarily intended to wait for IRQs and then poll completion queues, it
//! can also be used for notifying when a full submission queue can submit a new command (see
//! `AvailableSqEntryFuture`).

use std::fs::File;
use std::future::Future;
use std::io::prelude::*;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::{mem, task, thread};

use syscall::data::Event;
use syscall::flag::EVENT_READ;
use syscall::Result;

use crossbeam_channel::{Receiver, Sender};

use super::{CmdId, CqId, InterruptSources, Nvme, NvmeComp, NvmeCmd, SqId};

/// A notification request, sent by the future in order to tell the completion thread that the
/// current task wants a notification when a matching completion queue entry has been seen.
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
    }
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
    receiver: Receiver<NotifReq>,
    event_queue: File,
}
impl CqReactor {
    fn create_event_queue(int_sources: &InterruptSources) -> Result<File> {
        use syscall::flag::*;
        let fd = syscall::open("event:", O_CLOEXEC | O_RDWR)?;
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
        int_sources: InterruptSources,
        receiver: Receiver<NotifReq>,
    ) -> Result<Self> {
        Ok(Self {
            event_queue: Self::create_event_queue(&int_sources)?,
            int_sources,
            nvme,
            pending_reqs: Vec::new(),
            receiver,
        })
    }
    fn handle_notif_reqs(&mut self) {
        for req in self.receiver.try_iter() {
            match req {
                NotifReq::RequestCompletion {
                    sq_id,
                    cq_id,
                    cmd_id,
                    waker,
                    message,
                } => self.pending_reqs.push(PendingReq::PendingCompletion {
                    sq_id,
                    cq_id,
                    cmd_id,
                    message,
                    waker,
                }),
                NotifReq::RequestAvailSubmission { sq_id, waker } => self.pending_reqs.push(PendingReq::PendingAvailSubmission { sq_id, waker, }),
            }
        }
    }
    fn poll_completion_queues(&mut self, iv: u16) -> Option<()> {
        let ivs_read_guard = self.nvme.cqs_for_ivs.read().unwrap();
        let cqs_read_guard = self.nvme.completion_queues.read().unwrap();

        let mut entry_count = 0;

        for cq_id in ivs_read_guard.get(&iv)?.iter().copied() {
            let completion_queue_guard = cqs_read_guard.get(&cq_id)?.lock().unwrap();
            let &mut (ref mut completion_queue, _) = &mut *completion_queue_guard;

            let (head, entry) = match completion_queue.complete() {
                Some(e) => e,
                None => continue,
            };

            self.nvme.completion_queue_head(cq_id, head);

            self.nvme.submission_queues.read().unwrap().get(&entry.sq_id).expect("nvmed: internal error: queue returned from controller doesn't exist").lock().unwrap().head = entry.sq_head;

            self.try_notify_futures(cq_id, &entry);

            entry_count += 1;
        }
        if entry_count == 0 {}

        Some(())
    }
    fn finish_pending_completion(&mut self, req_cq_id: CqId, cq_id: CqId, sq_id: SqId, cmd_id: CmdId, entry: &NvmeComp, i: usize) -> bool {
        if req_cq_id == cq_id
            && sq_id == entry.sq_id
            && cmd_id == entry.cid
        {
            let (waker, message) = match self.pending_reqs.remove(i) {
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
    fn finish_pending_avail_submission(&mut self, sq_id: SqId, entry: &NvmeComp, i: usize) -> bool {
        if sq_id == entry.sq_id {
            let waker = match self.pending_reqs.remove(i) {
                PendingReq::PendingAvailSubmission { waker, .. } => waker,
                _ => unreachable!(),
            };
            waker.wake();

            true
        } else {
            false
        }
    }
    fn try_notify_futures(&mut self, cq_id: CqId, entry: &NvmeComp) -> Option<()> {
        let mut i = 0usize;

        let mut futures_notified = 0;

        while i < self.pending_reqs.len() {
            match &self.pending_reqs[i] {
                &PendingReq::PendingCompletion { cq_id: req_cq_id, sq_id, cmd_id, .. } => if self.finish_pending_completion(req_cq_id, cq_id, sq_id, cmd_id, entry, i) {
                    futures_notified += 1;
                } else {
                    i += 1;
                }
                &PendingReq::PendingAvailSubmission { sq_id, .. } => if self.finish_pending_avail_submission(sq_id, entry, i) {
                    futures_notified += 1;
                } else {
                    i += 1;
                }
            }
        }
        if futures_notified == 0 {}
        Some(())
    }

    fn run(mut self) {
        let mut event = Event::default();
        let mut irq_word = [0u8; 8]; // stores the IRQ count

        const WORD_SIZE: usize = mem::size_of::<usize>();

        loop {
            self.handle_notif_reqs();

            // block on getting the next event
            if self.event_queue.read(&mut event).unwrap() == 0 {
                // event queue has been destroyed
                break;
            }
            if event.flags & EVENT_READ != EVENT_READ {
                continue;
            }

            let (vector, irq_handle) = match self.int_sources.iter_mut().nth(event.id) {
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
    thread::spawn(move || {
        CqReactor::new(nvme, interrupt_sources, receiver)
            .expect("nvmed: failed to setup CQ reactor")
            .run()
    })
}

struct CompletionMessage {
    cq_entry: NvmeComp,
}

enum CompletionFutureState {
    // not really required, but makes futures inert
    Pending {
        sender: Sender<NotifReq>,
        cq_id: CqId,
        cmd_id: CmdId,
        sq_id: SqId,
        message: Arc<Mutex<Option<CompletionMessage>>>,
    },
    Finished,
}
pub struct CompletionFuture {
    state: CompletionFutureState,
}

// enum not self-referential
impl Unpin for CompletionFuture {}

impl Future for CompletionFuture {
    type Output = NvmeComp;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        let this = &mut self.get_mut().state;

        match this {
            &mut CompletionFutureState::Pending {
                message,
                cq_id,
                cmd_id,
                sq_id,
                sender,
            } => {
                if let Some(value) = message.lock().unwrap().take() {
                    *this = CompletionFutureState::Finished;
                    task::Poll::Ready(value.cq_entry)
                } else {
                    sender.send(NotifReq::RequestCompletion {
                        cq_id,
                        sq_id,
                        cmd_id,
                        waker: context.waker().clone(),
                        message: Arc::clone(&message),
                    });
                    task::Poll::Pending
                }
            }
            &mut CompletionFutureState::Finished => {
                panic!("calling poll() on an already finished CompletionFuture")
            }
        }
    }
}

impl Nvme {
    /// Returns a future representing an eventual completion queue event, in `cq_id`, from `sq_id`,
    /// with the individual command identified by `cmd_id`.
    pub fn completion(&self, sq_id: SqId, cmd_id: CmdId, cq_id: SqId) -> CompletionFuture {
        CompletionFuture {
            state: CompletionFutureState::Pending {
                sender: self.reactor_sender.clone(),
                cq_id,
                cmd_id,
                sq_id,
                message: Arc::new(Mutex::new(None)),
            },
        }
    }
    /// Returns a future representing a submission queue becoming non-full. Make sure that the
    /// queue doesn't have any additional free entries first though, so that the reactor doesn't
    /// have to interfere.
    pub fn wait_for_available_submission<'a, F: FnOnce(CmdId) -> NvmeCmd>(&'a self, sq_id: SqId, f: F) -> SubmissionFuture<'a, F> {
        SubmissionFuture {
            state: SubmissionFutureState::Pending {
                sq_id,
                cmd_init: f,
                nvme: &self,
            },
        }
    }
}

pub(crate) enum SubmissionFutureState<'a, F> {
    // the queue was known to be full when checked, thus the reactor is asked
    Pending {
        sq_id: SqId,
        cmd_init: F,
        nvme: &'a Nvme,
    },
    // returned when there was an available submission entry from the beginning
    Ready(CmdId),
    Finished,
}

/// A future representing a submission queue eventually becoming non-full. In most cases this
/// future will finish directly, since all entries in the queue have to be occupied for it to block.
pub struct SubmissionFuture<'a, F> {
    pub(crate) state: SubmissionFutureState<'a, F>,
}

impl<F> Unpin for SubmissionFuture<'_, F> {}

impl<F: FnOnce(CmdId) -> NvmeCmd> Future for SubmissionFuture<'_, F> {
    type Output = CmdId;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let state = &mut self.get_mut().state;

        match state {
            &mut SubmissionFutureState::Pending { sq_id, cmd_init, nvme } => match nvme.try_submit_command(sq_id, cmd_init) {
                Ok(cmd_id) => {
                    *state = SubmissionFutureState::Finished;
                    task::Poll::Ready(cmd_id)
                }
                Err(closure) => {
                    nvme.reactor_sender.send(NotifReq::RequestAvailSubmission { sq_id, waker: context.waker().clone() });
                    *state = SubmissionFutureState::Pending { sq_id, cmd_init: closure, nvme };
                    task::Poll::Pending
                }
            }
            &mut SubmissionFutureState::Ready(value) => {
                *state = SubmissionFutureState::Finished;
                task::Poll::Ready(value)
            }
            &mut SubmissionFutureState::Finished => panic!("calling poll() on an already finished SubmissionFuture"),
        }
    }
}
