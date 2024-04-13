use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::future::{Future, IntoFuture};
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::panic::UnwindSafe;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::task;

use event::{EventFlags, RawEventQueue};
use slab::Slab;

type EventUserData = usize;

use super::{CmdId, CqId, InterruptSources, Nvme, NvmeCmd, NvmeComp, SqId};

type FutIdx = usize;

/// Async executor, single IV, thread-per-core architecture
pub struct LocalExecutor {
    // TODO: Support multiple disks?
    nvme: Arc<Nvme>,

    queue: RawEventQueue,
    vector: u16,
    irq_handle: File,
    intx: bool,

    // TODO: One IV and SQ/CQ per core (where the admin queue can be managed by the main thread).
    awaiting_submission: RefCell<HashMap<SqId, VecDeque<FutIdx>>>,
    awaiting_completion: RefCell<HashMap<CqId, HashMap<CmdId, (FutIdx, NonNull<Option<NvmeComp>>)>>>,

    external_event: RefCell<HashMap<EventUserData, (FutIdx, NonNull<EventFlags>)>>,
    next_user_data: Cell<usize>,

    ready_futures: RefCell<VecDeque<FutIdx>>,
    futures: RefCell<Slab<Pin<Box<dyn Future<Output = ()> + 'static>>>>,
    is_polling: Cell<bool>,
}

thread_local! {
    static THE_EXECUTOR: RefCell<Option<Rc<LocalExecutor>>> = RefCell::new(None);
}

impl LocalExecutor {
    pub fn init(nvme: Arc<Nvme>, int_sources: InterruptSources) -> Rc<Self> {
        let queue = RawEventQueue::new().expect("failed to allocate event queue for local executor");

        // TODO: Multiple CPUs
        let intx = matches!(int_sources, InterruptSources::Intx(_));
        let (vector, irq_handle) = match int_sources {
            InterruptSources::Msi(mut ivs) => ivs.pop_first().map(|(a, b)| (u16::from(a), b)).unwrap(),
            InterruptSources::MsiX(mut ivs) => ivs.pop_first().unwrap(),
            InterruptSources::Intx(handle) => (0, handle),
        };
        queue.subscribe(irq_handle.as_raw_fd() as usize, 0, EventFlags::READ)
            .expect("failed to subscribe to IRQ event");

        let this = Rc::new(Self {
            nvme,

            queue,
            vector,
            intx,
            irq_handle,

            awaiting_submission: RefCell::new(HashMap::new()),
            awaiting_completion: RefCell::new(HashMap::new()),
            external_event: RefCell::new(HashMap::new()),
            next_user_data: Cell::new(1),
            ready_futures: RefCell::new(VecDeque::new()),
            futures: RefCell::new(Slab::with_capacity(16)),
            is_polling: Cell::new(false),
        });
        THE_EXECUTOR.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&this)));
        this
    }
    pub fn register_external_event(&self, fd: usize, flags: event::EventFlags) -> ExternalEventSource {
        let user_data = self.next_user_data.get();
        self.next_user_data.set(user_data.checked_add(1).unwrap());

        self.queue.subscribe(fd, user_data, flags)
            .expect("failed to subscribe to event");

        ExternalEventSource { flags: event::EventFlags::empty(), user_data, _not_send_or_unpin: PhantomData }
    }
    pub fn current() -> Rc<LocalExecutor> {
        THE_EXECUTOR.with(|e| Rc::clone(e.borrow().as_ref().unwrap()))
    }
    pub fn poll(&self) -> usize {
        assert!(!self.is_polling.replace(true));

        let mut finished = 0;

        for future_idx in self.ready_futures.borrow_mut().drain(..) {
            let waker = waker(future_idx);

            struct Wrapper<T>(T);
            impl<T> UnwindSafe for Wrapper<T> {}

            let mut futures = self.futures.borrow_mut();
            let future = Wrapper(futures[future_idx].as_mut());
            let res = match std::panic::catch_unwind(|| future.0.poll(&mut task::Context::from_waker(&waker))) {
                Ok(r) => r,
                Err(_) => {
                    log::error!("Task panicked!");
                    core::mem::forget(futures.remove(future_idx));
                    continue;
                }
            };
            if res.is_ready() {
                drop(futures.remove(future_idx));
                finished += 1;
            }
        }
        self.is_polling.set(false);

        finished
    }
    pub fn spawn(&self, fut: impl IntoFuture<Output = ()> + 'static) {
        let idx = self.futures.borrow_mut().insert(Box::pin(fut.into_future()));
        self.ready_futures.borrow_mut().push_back(idx);
    }
    pub fn block_on<'a, O: 'a>(&self, fut: impl IntoFuture<Output = O> + 'a) -> O {
        let retval = Rc::new(RefCell::new(None));

        let retval2 = Rc::clone(&retval);
        let idx = self.futures.borrow_mut().insert({
            let t1: Pin<Box<dyn Future<Output = ()> + 'a>> = Box::pin(async move {
                *retval2.borrow_mut() = Some(fut.await);
            });
            // SAFETY: Apart from the lifetimes, the types are exactly the same. We also know
            // block_on simply cannot return without having fully awaited and dropped the future,
            // even if that future panics (cf. the catch_unwind invocation).
            let t2: Pin<Box<dyn Future<Output = ()> + 'static>> = unsafe { std::mem::transmute(t1) };

            t2
        });

        self.ready_futures.borrow_mut().push_front(idx);

        loop {
            let finished = self.poll();
            if retval.borrow().is_some() {
                break;
            }
            if finished == 0 {
                self.react();
            }
        }

        let o = retval.borrow_mut().take().unwrap();
        o
    }
    fn react(&self) {
        let event = self.queue.next_event().expect("failed to get next event");

        if event.user_data != 0 {
            let Some((fut_idx, flags_ptr)) = self.external_event.borrow_mut().remove(&event.user_data) else {
                // Spurious event
                return;
            };
            unsafe {
                flags_ptr.as_ptr().write(event::EventFlags::from_bits_retain(event.flags));
            }
            self.ready_futures.borrow_mut().push_back(fut_idx);
            return;
        }

        if self.intx {
            let mut buf = [0_u8; core::mem::size_of::<usize>()];
            if (&self.irq_handle).read(&mut buf).unwrap() != 0 {
                (&self.irq_handle).write(&buf).unwrap();
            }
        }

        let ctxt = self.nvme.cur_thread_ctxt();
        let mut ctxt = ctxt.lock();
        let ctxt = &mut *ctxt;

        // TODO: The kernel should probably do the masking, which should happen before EOI
        // messages to the interrupt controller.
        self.nvme.set_vector_masked(self.vector, true);
        for (sq_cq_id, (sq, cq)) in ctxt.queues.iter_mut() {
            let mut head = None;

            while let Some((new_head, cqe)) = cq.complete() {
                log::trace!("new head {new_head} cqe {cqe:?}");
                if let Some((fut_idx, comp_ptr)) = self.awaiting_completion.borrow_mut().get_mut(sq_cq_id).and_then(|per_cmd| per_cmd.remove(&{ cqe.cid })) {
                    unsafe {
                        comp_ptr.as_ptr().write(Some(cqe));
                    }
                    self.ready_futures.borrow_mut().push_back(fut_idx);
                    sq.head = cqe.sq_head;
                }
                head = Some(new_head);
            }

            if let Some(head) = head {
                unsafe {
                    self.nvme.completion_queue_head(*sq_cq_id, head);
                }
            }
        }
        self.nvme.set_vector_masked(self.vector, false);
    }
}

pub struct NvmeFuture {
    pub(crate) state: State,
    pub(crate) comp: Option<NvmeComp>,
    pub(crate) _not_send: PhantomData<*const ()>,
}
pub(crate) enum State {
    Submitting { sq_id: SqId, cmd: NvmeCmd },
    Completing { cq_id: CqId, cmd_id: CmdId },
}

fn current_executor_and_idx(cx: &mut task::Context<'_>) -> (Rc<LocalExecutor>, FutIdx) {
    let executor = LocalExecutor::current();

    let idx = cx.waker().as_raw().data() as FutIdx;
    assert_eq!(cx.waker().as_raw().vtable() as *const _, &THIS_VTABLE, "incompatible executor for NvmeFuture");

    (executor, idx)
}

impl Future for NvmeFuture {
    type Output = NvmeComp;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        let (executor, idx) = current_executor_and_idx(cx);

        match this.state {
            State::Submitting { sq_id, mut cmd } => {
                let mut awaiting = executor.awaiting_submission.borrow_mut();

                if let Some((cq_id, cmd_id)) = executor.nvme.try_submit_raw(&mut executor.nvme.cur_thread_ctxt().lock(), sq_id, |cmd_id| {
                    cmd.cid = cmd_id;
                    log::trace!("About to submit {cmd:?}");
                    cmd
                }, || {
                    awaiting.entry(sq_id).or_default().push_back(idx);
                }) {
                    executor.awaiting_completion.borrow_mut().entry(cq_id).or_default().insert(cmd_id, (idx, (&mut this.comp).into()));
                    this.state = State::Completing { cq_id, cmd_id };
                }
                task::Poll::Pending
            }
            State::Completing { cq_id, cmd_id } => match this.comp.take() {
                Some(comp) => {
                    log::trace!("ready!");
                    task::Poll::Ready(comp)
                }

                // Shouldn't technically be possible
                None => {
                    log::trace!("spurious poll");
                    executor.awaiting_completion.borrow_mut().entry(cq_id).or_default().insert(cmd_id, (idx, (&mut this.comp).into()));
                    task::Poll::Pending
                }
            }
        }
    }
}

unsafe fn vt_clone(idx: *const ()) -> task::RawWaker { task::RawWaker::new(idx, &THIS_VTABLE) }
unsafe fn vt_drop(_idx: *const ()) {}
unsafe fn vt_wake(idx: *const ()) {
    THE_EXECUTOR.with(|exec| exec.borrow().as_ref().unwrap().ready_futures.borrow_mut().push_back(idx as FutIdx));
}

static THIS_VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(vt_clone, vt_wake, vt_wake, vt_drop);
fn waker(idx: FutIdx) -> task::Waker {
    unsafe { task::Waker::from_raw(task::RawWaker::new(idx as *const (), &THIS_VTABLE)) }
}

pub struct ExternalEventSource {
    flags: event::EventFlags,
    user_data: EventUserData,
    _not_send_or_unpin: PhantomData<*const ()>,
}
pub struct Event {
    flags: event::EventFlags,
    _not_send: PhantomData<*const ()>,
}
impl ExternalEventSource {
    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Option<Event>> {
        let this = unsafe { self.get_unchecked_mut() };

        let flags = std::mem::take(&mut this.flags);

        if flags.is_empty() {
            let (executor, idx) = current_executor_and_idx(cx);
            executor.external_event.borrow_mut().insert(this.user_data, (idx, (&mut this.flags).into()));
            return task::Poll::Pending;
        }
        task::Poll::Ready(Some(Event {
            flags,
            _not_send: PhantomData,
        }))
    }
    pub async fn next(mut self: Pin<&mut Self>) -> Option<Event> {
        core::future::poll_fn(|cx| self.as_mut().poll_next(cx)).await
    }
}
