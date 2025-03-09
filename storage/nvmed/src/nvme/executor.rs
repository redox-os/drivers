use std::cell::RefCell;
use std::fs::File;
use std::rc::Rc;
use std::sync::Arc;

use executor::{Hardware, LocalExecutor};

use super::{CmdId, CqId, Nvme, NvmeCmd, NvmeComp, SqId};

pub struct NvmeHw;

impl Hardware for NvmeHw {
    type Iv = u16;
    type Sqe = NvmeCmd;
    type Cqe = NvmeComp;
    type CmdId = CmdId;
    type CqId = CqId;
    type SqId = SqId;
    type GlobalCtxt = Arc<Nvme>;

    fn mask_vector(ctxt: &Arc<Nvme>, iv: Self::Iv) {
        ctxt.set_vector_masked(iv, true)
    }
    fn unmask_vector(ctxt: &Arc<Nvme>, iv: Self::Iv) {
        ctxt.set_vector_masked(iv, false)
    }
    fn set_sqe_cmdid(sqe: &mut NvmeCmd, id: CmdId) {
        sqe.cid = id;
    }
    fn get_cqe_cmdid(cqe: &Self::Cqe) -> Self::CmdId {
        cqe.cid
    }
    fn vtable() -> &'static std::task::RawWakerVTable {
        &VTABLE
    }
    fn current() -> std::rc::Rc<executor::LocalExecutor<Self>> {
        THE_EXECUTOR.with(|exec| Rc::clone(exec.borrow().as_ref().unwrap()))
    }
    fn try_submit(
        nvme: &Arc<Nvme>,
        sq_id: Self::SqId,
        success: impl FnOnce(Self::CmdId) -> Self::Sqe,
        fail: impl FnOnce(),
    ) -> Option<(Self::CqId, Self::CmdId)> {
        let ctxt = nvme.cur_thread_ctxt();
        let ctxt = ctxt.lock();

        nvme.try_submit_raw(&*ctxt, sq_id, success, fail)
    }
    fn poll_cqes(nvme: &Arc<Nvme>, mut handle: impl FnMut(Self::CqId, Self::Cqe)) {
        let ctxt = nvme.cur_thread_ctxt();
        let ctxt = ctxt.lock();

        for (sq_cq_id, (sq, cq)) in ctxt.queues.borrow_mut().iter_mut() {
            while let Some((new_head, cqe)) = cq.complete() {
                unsafe {
                    nvme.completion_queue_head(*sq_cq_id, new_head);
                }
                sq.head = cqe.sq_head;
                log::trace!("new head {new_head} cqe {cqe:?}");
                handle(*sq_cq_id, cqe);
            }
        }
    }
    fn sq_cq(_ctxt: &Arc<Nvme>, id: Self::CqId) -> Self::SqId {
        id
    }
}

static VTABLE: std::task::RawWakerVTable = executor::vtable::<NvmeHw>();

thread_local! {
    static THE_EXECUTOR: RefCell<Option<Rc<LocalExecutor<NvmeHw>>>> = RefCell::new(None);
}

pub type NvmeExecutor = LocalExecutor<NvmeHw>;

pub fn init(nvme: Arc<Nvme>, iv: u16, intx: bool, irq_handle: File) -> Rc<LocalExecutor<NvmeHw>> {
    let this = Rc::new(executor::init_raw(nvme, iv, intx, irq_handle));
    THE_EXECUTOR.with(|exec| *exec.borrow_mut() = Some(Rc::clone(&this)));
    this
}
