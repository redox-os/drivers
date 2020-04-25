use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::prelude::*;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::{io, task, thread};
use std::os::unix::io::{FromRawFd, RawFd};

use syscall::Result;

use crossbeam_channel::{Sender, Receiver};

use crate::nvme::{InterruptMethod, Nvme, NvmeComp};

/// A source of interrupts. The NVME spec splits the definition of MSI into "single msi" and "multi
/// msi".
#[derive(Debug)]
pub enum IntSources {
    Intx(File),
    SingleMsi(File),
    MultiMsi(BTreeMap<u8, File>),
    MsiX(BTreeMap<u16, File>),
}

/// A notification request, sent by the future in order to tell the completion thread that the
/// current task wants a notification when a matching completion queue entry has been seen.
pub enum NotifReq {
    RequestCompletion {
        queue_id: usize,
        waker: task::Waker,
        // TODO: Get rid of this allocation
        message: Arc<Mutex<Option<CompletionMessage>>>,
    },
}

struct PendingReq {
    waker: task::Waker,
    message: Arc<Mutex<Option<CompletionMessage>>>,
    queue_id: usize,
}
struct CqReactor {
    int_sources: Option<IntSources>,
    nvme: Arc<Nvme>,
    pending_reqs: Vec<PendingReq>,
    receiver: Receiver<NotifReq>,
    event_queue: File,
}
impl CqReactor {
    fn create_event_queue() -> Result<File> {
        use syscall::flag::*;
        let fd = syscall::open("event:", O_CLOEXEC | O_RDWR)?;
        let mut file = unsafe { File::from_raw_fd(fd as RawFd) };
        todo!()
    }
    fn new(nvme: Arc<Nvme>, receiver: Receiver<NotifReq>) -> Result<Self> {
        Ok(Self {
            int_sources: None, // TODO
            nvme,
            pending_reqs: Vec::new(),
            receiver,
            event_queue: Self::create_event_queue()?,
        })
    }
    fn handle_notif_reqs(&mut self) {
        for req in self.receiver.try_iter() {
            match req {
                NotifReq::RequestCompletion { queue_id, waker, message } => self.pending_reqs.push(PendingReq {
                    queue_id,
                    message,
                    waker,
                }),
            }
        }
    }
    fn run(mut self) -> ! {
        loop {
            self.handle_notif_reqs();
        }
    }
}

pub fn start_cq_reactor_thread(nvme: Arc<Nvme>, receiver: Receiver<NotifReq>) -> thread::JoinHandle<()> {
    // Actually, nothing prevents us from spawning additional threads. the channel is MPMC and
    // everything is properly synchronized. I'm not saying this is strictly required, but with
    // multiple completion queues it might actually be worth considering.
    thread::spawn(move || {
        CqReactor::new(nvme, receiver)
            .expect("nvmed: failed to setup CQ reactor")
            .run()
    })
}

pub struct CompletionMessage {
    cq_entry: NvmeComp,
}

enum CompletionFuture {
    // not really required, but makes futures inert
    Init {
        sender: Sender<NotifReq>,
        queue_id: usize,
    },
    Pending {
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
            &mut Self::Init { sender, queue_id } => {
                let message = Arc::new(Mutex::new(None));
                sender.send(NotifReq::RequestCompletion {
                    queue_id,
                    waker: context.waker().clone(),
                    message: Arc::clone(&message),
                });
                *this = CompletionFuture::Pending {
                    message,
                };
                task::Poll::Pending
            }
            &mut Self::Pending { message } => if let Some(value) = message.lock().unwrap().take() {
                *this = Self::Finished;
                task::Poll::Ready(value.cq_entry)
            } else {
                // woken up but the reactor hadn't sent the message.
                // this is ideally unreachable
                task::Poll::Pending
            }
            &mut Self::Finished => panic!("calling poll() on an already finished CompletionFuture"),
        }
    }
}


impl Nvme {
    pub fn completion(&self, cq_id: usize) -> impl Future<Output = NvmeComp> + '_ {
        CompletionFuture::Init {
            sender: self.reactor_sender.clone(),
            queue_id: cq_id,
        }
    }
}
