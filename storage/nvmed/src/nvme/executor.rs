use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::future::{Future, IntoFuture};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::panic::UnwindSafe;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::task;

use event::{EventFlags, EventQueue, RawEventQueue};
use slab::Slab;

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
    awaiting_submission: HashMap<SqId, VecDeque<FutIdx>>,
    awaiting_completion: HashMap<CqId, HashMap<CmdId, (FutIdx, NonNull<NvmeComp>)>>,
    ready_comp: HashMap<FutIdx, NvmeComp>,
    ready_futures: VecDeque<FutIdx>,
    futures: Slab<Pin<Box<dyn Future<Output = ()> + 'static>>>
}

thread_local! {
    static THE_EXECUTOR: RefCell<Option<Rc<RefCell<LocalExecutor>>>> = RefCell::new(None);
}

impl LocalExecutor {
    pub fn init(nvme: Arc<Nvme>, int_sources: InterruptSources) -> Rc<RefCell<Self>> {
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

        let this = Rc::new(RefCell::new(Self {
            nvme,

            queue,
            vector,
            intx,
            irq_handle,

            awaiting_submission: HashMap::new(),
            awaiting_completion: HashMap::new(),
            ready_comp: HashMap::new(),
            ready_futures: VecDeque::new(),
            futures: Slab::with_capacity(16),
        }));
        THE_EXECUTOR.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&this)));
        this
    }
    pub fn current() -> Rc<RefCell<LocalExecutor>> {
        THE_EXECUTOR.with(|e| Rc::clone(e.borrow().as_ref().unwrap()))
    }
    pub fn poll(&mut self) {
        for future_idx in self.ready_futures.drain(..) {
            let waker = waker(future_idx);

            struct Wrapper<T>(T);
            impl<T> UnwindSafe for Wrapper<T> {}

            let future = Wrapper(self.futures[future_idx].as_mut());
            let res = match std::panic::catch_unwind(|| future.0.poll(&mut task::Context::from_waker(&waker))) {
                Ok(r) => r,
                Err(_) => {
                    log::error!("Task panicked!");
                    core::mem::forget(self.futures.remove(future_idx));
                    continue;
                }
            };
            if res.is_ready() {
                drop(self.futures.remove(future_idx));
            }
        }
    }
    pub fn spawn(&mut self, fut: impl IntoFuture<Output = ()> + 'static) {
        let idx = self.futures.insert(Box::pin(fut.into_future()));
        self.ready_futures.push_back(idx);
    }
    pub fn block_on<'a, O: 'a>(&mut self, fut: impl IntoFuture<Output = O> + 'a) -> O {
        let retval = Rc::new(RefCell::new(None));

        let retval2 = Rc::clone(&retval);
        let idx = self.futures.insert({
            let t1: Pin<Box<dyn Future<Output = ()> + 'a>> = Box::pin(async move {
                *retval2.borrow_mut() = Some(fut.await);
            });
            // SAFETY: Apart from the lifetimes, the types are exactly the same. We also know
            // block_on simply cannot return without having fully awaited and dropped the future,
            // even if that future panics (cf. the catch_unwind invocation).
            let t2: Pin<Box<dyn Future<Output = ()> + 'static>> = unsafe { std::mem::transmute(t1) };

            t2
        });

        self.ready_futures.push_front(idx);

        while retval.borrow().is_none() {
            self.poll();
            self.react();
        }

        let o = retval.borrow_mut().take().unwrap();
        o
    }
    fn react(&mut self) {
        let _ = self.queue.next_event().expect("failed to get next event");

        if self.intx {
            let mut buf = [0_u8; core::mem::size_of::<usize>()];
            if self.irq_handle.read(&mut buf).unwrap() != 0 {
                self.irq_handle.write(&buf).unwrap();
            }
        }

        // TODO: The kernel should probably do the masking, which should happen before EOI
        // messages to the interrupt controller.
        self.nvme.set_vector_masked(self.vector, true);

        todo!();

        self.nvme.set_vector_masked(self.vector, false);
    }
}

pub struct NvmeFuture {
    state: State,
    // TODO: future memory models may not like this
    comp: NvmeComp,
}
enum State {
    Submitting { sq_id: SqId, cmd: NvmeCmd },
    Completing { cq_id: CqId, cmd_id: CmdId },
}
impl Future for NvmeFuture {
    type Output = NvmeComp;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        let executor = LocalExecutor::current();
        let mut executor = executor.borrow_mut();
        let executor = &mut *executor;

        let idx = cx.waker().as_raw().data() as FutIdx;
        assert_eq!(cx.waker().as_raw().vtable() as *const _, &THIS_VTABLE, "incompatible executor for NvmeFuture");

        match this.state {
            State::Submitting { sq_id, mut cmd } => {
                let awaiting = &mut executor.awaiting_submission;

                if let Some((cq_id, cmd_id)) = executor.nvme.try_submit_raw(sq_id, |cmd_id| {
                    cmd.cid = cmd_id;
                    cmd
                }, || {
                    awaiting.entry(sq_id).or_default().push_back(idx);
                }) {
                    executor.awaiting_completion.entry(cq_id).or_default().insert(cmd_id, (idx, (&mut this.comp).into()));
                    this.state = State::Completing { cq_id, cmd_id };
                }
                task::Poll::Pending
            }
            State::Completing { cq_id, cmd_id } => match executor.ready_comp.remove(&idx) {
                Some(comp) => task::Poll::Ready(comp),

                // Shouldn't technically occur
                None => {
                    executor.awaiting_completion.entry(cq_id).or_default().insert(cmd_id, (idx, (&mut this.comp).into()));
                    task::Poll::Pending
                }
            }
        }
    }
}

unsafe fn vt_clone(idx: *const ()) -> task::RawWaker { task::RawWaker::new(idx, &THIS_VTABLE) }
unsafe fn vt_drop(_idx: *const ()) {}
unsafe fn vt_wake(idx: *const ()) {
    THE_EXECUTOR.with(|exec| exec.borrow().as_ref().unwrap().borrow_mut().ready_futures.push_back(idx as FutIdx));
}

static THIS_VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(vt_clone, vt_wake, vt_wake, vt_drop);
fn waker(idx: FutIdx) -> task::Waker {
    unsafe { task::Waker::from_raw(task::RawWaker::new(idx as *const (), &THIS_VTABLE)) }
}
