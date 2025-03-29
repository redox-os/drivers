use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt::Debug;
use std::fs::File;
use std::future::{Future, IntoFuture};
use std::hash::Hash;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task;

use event::{EventFlags, RawEventQueue};
use slab::Slab;

type EventUserData = usize;

type FutIdx = usize;

pub trait Hardware: Sized {
    type CmdId: Clone + Copy + Debug + Hash + Eq + PartialEq;
    type CqId: Clone + Copy + Debug + Hash + Eq + PartialEq;
    type SqId: Clone + Copy + Debug + Hash + Eq + PartialEq;
    type Sqe: Debug + Clone + Copy;
    type Cqe;
    type Iv: Clone + Copy + Debug;

    type GlobalCtxt;

    // TODO: the kernel should also do this automatically before sending EOI messages to the IC
    fn mask_vector(ctxt: &Self::GlobalCtxt, iv: Self::Iv);
    fn unmask_vector(ctxt: &Self::GlobalCtxt, iv: Self::Iv);

    fn set_sqe_cmdid(sqe: &mut Self::Sqe, id: Self::CmdId);
    fn get_cqe_cmdid(cqe: &Self::Cqe) -> Self::CmdId;

    // TODO: support multiple SQs per CQ or vice versa?
    fn sq_cq(ctxt: &Self::GlobalCtxt, id: Self::CqId) -> Self::SqId;

    fn current() -> Rc<LocalExecutor<Self>>;
    fn vtable() -> &'static task::RawWakerVTable;

    fn try_submit(
        ctxt: &Self::GlobalCtxt,
        sq_id: Self::SqId,
        success: impl FnOnce(Self::CmdId) -> Self::Sqe,
        fail: impl FnOnce(),
    ) -> Option<(Self::CqId, Self::CmdId)>;
    fn poll_cqes(ctxt: &Self::GlobalCtxt, handle: impl FnMut(Self::CqId, Self::Cqe));
}

/// Async executor, single IV, thread-per-core architecture
pub struct LocalExecutor<Hw: Hardware> {
    global_ctxt: Hw::GlobalCtxt,

    queue: RawEventQueue,
    vector: Hw::Iv,
    irq_handle: File,
    intx: bool,

    // TODO: One IV and SQ/CQ per core (where the admin queue can be managed by the main thread).
    awaiting_submission: RefCell<HashMap<Hw::SqId, VecDeque<FutIdx>>>,
    awaiting_completion:
        RefCell<HashMap<Hw::CqId, HashMap<Hw::CmdId, (FutIdx, NonNull<Option<Hw::Cqe>>)>>>,

    external_event: RefCell<HashMap<EventUserData, (FutIdx, NonNull<EventFlags>)>>,
    next_user_data: Cell<usize>,

    ready_futures: RefCell<VecDeque<FutIdx>>,
    futures: RefCell<Slab<Pin<Box<dyn Future<Output = ()> + 'static>>>>,
    is_polling: Cell<bool>,
}

impl<Hw: Hardware> LocalExecutor<Hw> {
    pub fn register_external_event(
        &self,
        fd: usize,
        flags: event::EventFlags,
    ) -> ExternalEventSource<Hw> {
        let user_data = self.next_user_data.get();
        self.next_user_data.set(user_data.checked_add(1).unwrap());

        self.queue
            .subscribe(fd, user_data, flags)
            .expect("failed to subscribe to event");

        ExternalEventSource {
            flags: event::EventFlags::empty(),
            user_data,
            _not_send_or_unpin: PhantomData,
        }
    }
    pub fn current() -> Rc<Self> {
        Hw::current()
    }
    pub fn poll(&self) -> usize {
        assert!(!self.is_polling.replace(true));

        let mut finished = 0;

        for future_idx in self.ready_futures.borrow_mut().drain(..) {
            let waker = waker::<Hw>(future_idx);

            let mut futures = self.futures.borrow_mut();
            let res = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                futures[future_idx]
                    .as_mut()
                    .poll(&mut task::Context::from_waker(&waker))
            })) {
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
        let idx = self
            .futures
            .borrow_mut()
            .insert(Box::pin(fut.into_future()));
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
            let t2: Pin<Box<dyn Future<Output = ()> + 'static>> =
                unsafe { std::mem::transmute(t1) };

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
            let Some((fut_idx, flags_ptr)) =
                self.external_event.borrow_mut().remove(&event.user_data)
            else {
                // Spurious event
                return;
            };
            unsafe {
                flags_ptr
                    .as_ptr()
                    .write(event::EventFlags::from_bits_retain(event.flags));
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

        // TODO: The kernel should probably do the masking (when using MSI/MSI-X at least), which
        // should happen before EOI messages to the interrupt controller.
        Hw::mask_vector(&self.global_ctxt, self.vector);
        Hw::poll_cqes(&self.global_ctxt, |cq_id, cqe| {
            if let Some((fut_idx, comp_ptr)) = self
                .awaiting_completion
                .borrow_mut()
                .get_mut(&cq_id)
                .and_then(|per_cmd| per_cmd.remove(&Hw::get_cqe_cmdid(&cqe)))
            {
                unsafe {
                    comp_ptr.as_ptr().write(Some(cqe));
                }
                self.ready_futures.borrow_mut().push_back(fut_idx);

                if let Some(submitting) = self
                    .awaiting_submission
                    .borrow_mut()
                    .get_mut(&Hw::sq_cq(&self.global_ctxt, cq_id))
                    .and_then(|q| q.pop_front())
                {
                    self.ready_futures.borrow_mut().push_back(submitting);
                }
            }
        });
        Hw::unmask_vector(&self.global_ctxt, self.vector);
    }
    pub async fn submit(&self, sq_id: Hw::SqId, cmd: Hw::Sqe) -> Hw::Cqe {
        CqeFuture::<Hw> {
            state: State::<Hw>::Submitting { sq_id, cmd },
            comp: None,
            _not_send: PhantomData,
        }
        .await
    }
}

struct CqeFuture<Hw: Hardware> {
    pub state: State<Hw>,
    pub comp: Option<Hw::Cqe>,
    pub _not_send: PhantomData<*const ()>,
}
enum State<Hw: Hardware> {
    Submitting { sq_id: Hw::SqId, cmd: Hw::Sqe },
    Completing { cq_id: Hw::CqId, cmd_id: Hw::CmdId },
}

fn current_executor_and_idx<Hw: Hardware>(
    cx: &mut task::Context<'_>,
) -> (Rc<LocalExecutor<Hw>>, FutIdx) {
    let executor = LocalExecutor::current();

    let idx = cx.waker().data() as FutIdx;
    assert_eq!(
        cx.waker().vtable() as *const _,
        Hw::vtable(),
        "incompatible executor for CqeFuture"
    );

    (executor, idx)
}

impl<Hw: Hardware> Future for CqeFuture<Hw> {
    type Output = Hw::Cqe;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        let (executor, idx) = current_executor_and_idx::<Hw>(cx);

        match this.state {
            State::Submitting { sq_id, mut cmd } => {
                let mut awaiting = executor.awaiting_submission.borrow_mut();

                if let Some((cq_id, cmd_id)) = Hw::try_submit(
                    &executor.global_ctxt,
                    sq_id,
                    |cmd_id| {
                        Hw::set_sqe_cmdid(&mut cmd, cmd_id);
                        log::trace!("About to submit {cmd:?}");
                        cmd
                    },
                    || {
                        awaiting.entry(sq_id).or_default().push_back(idx);
                    },
                ) {
                    executor
                        .awaiting_completion
                        .borrow_mut()
                        .entry(cq_id)
                        .or_default()
                        .insert(cmd_id, (idx, (&mut this.comp).into()));
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
                    executor
                        .awaiting_completion
                        .borrow_mut()
                        .entry(cq_id)
                        .or_default()
                        .insert(cmd_id, (idx, (&mut this.comp).into()));
                    task::Poll::Pending
                }
            },
        }
    }
}

unsafe fn vt_clone<Hw: Hardware>(idx: *const ()) -> task::RawWaker {
    task::RawWaker::new(idx, Hw::vtable())
}
unsafe fn vt_drop(_idx: *const ()) {}
unsafe fn vt_wake<Hw: Hardware>(idx: *const ()) {
    Hw::current()
        .ready_futures
        .borrow_mut()
        .push_back(idx as FutIdx);
}

fn waker<Hw: Hardware>(idx: FutIdx) -> task::Waker {
    unsafe { task::Waker::from_raw(task::RawWaker::new(idx as *const (), Hw::vtable())) }
}
pub const fn vtable<Hw: Hardware>() -> task::RawWakerVTable {
    task::RawWakerVTable::new(vt_clone::<Hw>, vt_wake::<Hw>, vt_wake::<Hw>, vt_drop)
}

pub struct ExternalEventSource<Hw: Hardware> {
    flags: event::EventFlags,
    user_data: EventUserData,
    _not_send_or_unpin: PhantomData<(*const (), fn() -> Hw)>,
}
pub struct Event {
    flags: event::EventFlags,
    _not_send: PhantomData<*const ()>,
}
impl Event {
    pub fn flags(&self) -> event::EventFlags {
        self.flags
    }
}
impl<Hw: Hardware> ExternalEventSource<Hw> {
    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Option<Event>> {
        let this = unsafe { self.get_unchecked_mut() };

        let flags = std::mem::take(&mut this.flags);

        if flags.is_empty() {
            let (executor, idx) = current_executor_and_idx::<Hw>(cx);
            executor
                .external_event
                .borrow_mut()
                .insert(this.user_data, (idx, (&mut this.flags).into()));
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
pub fn init_raw<Hw: Hardware>(
    global_ctxt: Hw::GlobalCtxt,
    vector: Hw::Iv,
    intx: bool,
    irq_handle: File,
) -> LocalExecutor<Hw> {
    let queue = RawEventQueue::new().expect("failed to allocate event queue for local executor");

    // TODO: Multiple CPUs
    queue
        .subscribe(irq_handle.as_raw_fd() as usize, 0, EventFlags::READ)
        .expect("failed to subscribe to IRQ event");

    LocalExecutor {
        global_ctxt,

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
    }
}
