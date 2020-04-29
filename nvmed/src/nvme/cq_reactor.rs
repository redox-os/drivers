//! The Completion Queue Reactor. Functions like any other async/await reactor, but are driven by
//! IRQs triggering wakeups in order to poll NVME completion queues.

use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::prelude::*;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::{io, mem, task, thread};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

use syscall::Result;
use syscall::data::Event;
use syscall::flag::EVENT_READ;

use crossbeam_channel::{Sender, Receiver};

use crate::nvme::{CqId, CmdId, InterruptMethod, InterruptSources, Nvme, NvmeComp, NvmeCompQueue, SqId};

/// A notification request, sent by the future in order to tell the completion thread that the
/// current task wants a notification when a matching completion queue entry has been seen.
pub enum NotifReq {
    RequestCompletion {
        cq_id: CqId,
        sq_id: SqId,
        cmd_id: CmdId,

        waker: task::Waker,

        // TODO: Get rid of this allocation, or maybe a thread-local vec for reusing.
        message: Arc<Mutex<Option<CompletionMessage>>>,
    },
}

struct PendingReq {
    waker: task::Waker,
    message: Arc<Mutex<Option<CompletionMessage>>>,
    cq_id: u16,
    sq_id: u16,
    cmd_id: u16,
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
            if file.write(&Event {
                id: irq_handle.as_raw_fd() as usize,
                flags: syscall::EVENT_READ,
                data: num as usize,
            }).unwrap() == 0 {
                panic!("Failed to setup event queue for {} {:?}", num, irq_handle);
            }
        }
        Ok(file)
    }
    fn new(nvme: Arc<Nvme>, int_sources: InterruptSources, receiver: Receiver<NotifReq>) -> Result<Self> {
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
                NotifReq::RequestCompletion { sq_id, cq_id, cmd_id, waker, message } => self.pending_reqs.push(PendingReq {
                    sq_id,
                    cq_id,
                    cmd_id,
                    message,
                    waker,
                }),
            }
        }
    }
    fn poll_completion_queues(&mut self, iv: u16) -> Option<()> {
        let ivs_read_guard = self.nvme.cqs_for_ivs.read().unwrap();
        let cqs_read_guard = self.nvme.completion_queues.read().unwrap();

        let mut entry_count = 0;

        for cq_id in ivs_read_guard.get(&iv)?.iter() {
            let completion_queue_guard = cqs_read_guard.get(cq_id)?.lock().unwrap();
            let completion_queue: &mut NvmeCompQueue = &mut *completion_queue_guard;

            let (head, entry) = match completion_queue.complete() {
                Some(e) => e,
                None => continue,
            };

            self.nvme.completion_queue_head(cq_id, head);

            self.try_notify_futures(cq_id, &entry);

            entry_count += 1;
        }
        if entry_count == 0 {

        }

        Some(())
    }
    fn try_notify_futures(&mut self, cq_id: CqId, entry: &NvmeComp) -> Option<()> {
        let mut i = 0usize;

        let mut futures_notified = 0;

        while i < self.pending_reqs.len() {
            let pending_req = &self.pending_reqs[i];

            if pending_req.cq_id == cq_id && pending_req.sq_id == entry.sq_id && pending_req.cid == entry.cmd_id {
                let pending_req_owned = self.pending_reqs.remove(i);

                *pending_req_owned.message.lock().unwrap() = Some(*entry);
                pending_req_owned.waker.wake();

                futures_notified += 1;
            } else {
                i += 1;
            }
        }
        if futures_notified == 0 {
        }
    }

    fn run(mut self) {
        let mut event = Event::default();
        let mut irq_word = [0u8; 8]; // stores the IRQ count

        const WORD_SIZE: usize = mem::size_of::<usize>();

        loop {
            self.handle_notif_reqs();

            // block on getting the next event
            if self.event_queue.read(&mut event) == 0 {
                // event queue has been destroyed
                break;
            }
            if event.flags & EVENT_READ == 0 {
                continue;
            }

            let (vector, irq_handle) = match self.int_sources.get_mut().nth(event.id) {
                Some(s) => s,
                None => continue,
            };
            if irq_handle.read(&mut irq_word[..WORD_SIZE]) == 0 {
                continue;
            }
            // acknowledge the interrupt (only necessary for level-triggered INTx# interrups)
            if irq_handle.write(&irq_word[..WORD_SIZE]) == 0 {
                continue;
            }

            self.poll_completion_queues(vector);
        }
    }
}

pub fn start_cq_reactor_thread(nvme: Arc<Nvme>, interrupt_sources: InterruptSources, receiver: Receiver<NotifReq>) -> thread::JoinHandle<()> {
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

enum CompletionFuture {
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

// enum not self-referential
impl Unpin for CompletionFuture {}

impl Future for CompletionFuture {
    type Output = NvmeComp;

    fn poll(self: Pin<&mut Self>, context: &mut task::Context) -> task::Poll<Self::Output> {
        let this = self.get_mut();

        match this {
            &mut Self::Pending { message, cq_id, cmd_id, sq_id, sender } => if let Some(value) = message.lock().unwrap().take() {
                *this = Self::Finished;
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
            &mut Self::Finished => panic!("calling poll() on an already finished CompletionFuture"),
        }
    }
}


impl Nvme {
    /// Returns a future representing an eventual completion queue event, in `cq_id`, from `sq_id`,
    /// with the individual command identified by `cmd_id`.
    pub fn completion(&self, sq_id: SqId, cmd_id: CmdId, cq_id: SqId) -> impl Future<Output = NvmeComp> + '_ {
        CompletionFuture::Pending {
            sender: self.reactor_sender.clone(),
            cq_id,
            cmd_id,
            sq_id,
            message: Arc::new(Mutex::new(None)),
        }
    }
}
