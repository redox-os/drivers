use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use std::fs::File;
use std::iter;
use std::sync::atomic::AtomicU16;
use std::sync::Arc;

use parking_lot::{Mutex, ReentrantMutex, RwLock};

use common::io::{Io, Mmio};
use syscall::error::{Error, Result, EIO};

use common::dma::Dma;

pub mod cmd;
pub mod executor;
pub mod identify;
pub mod queues;

use self::executor::NvmeExecutor;
pub use self::queues::{NvmeCmd, NvmeCmdQueue, NvmeComp, NvmeCompQueue};

use pcid_interface::msi::{MappedMsixRegs, MsiInfo};
use pcid_interface::PciFunctionHandle;

/// Used in conjunction with `InterruptMethod`, primarily by the CQ executor.
#[derive(Debug)]
pub enum InterruptSources {
    MsiX(BTreeMap<u16, File>),
    Msi(BTreeMap<u8, File>),
    Intx(File),
}
impl InterruptSources {
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u16, &mut File)> {
        use std::collections::btree_map::IterMut as BTreeIterMut;
        use std::iter::Once;

        enum IterMut<'a> {
            Msi(BTreeIterMut<'a, u8, File>),
            MsiX(BTreeIterMut<'a, u16, File>),
            Intx(Once<&'a mut File>),
        }
        impl<'a> Iterator for IterMut<'a> {
            type Item = (u16, &'a mut File);

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    &mut Self::Msi(ref mut iter) => iter
                        .next()
                        .map(|(&vector, handle)| (u16::from(vector), handle)),
                    &mut Self::MsiX(ref mut iter) => {
                        iter.next().map(|(&vector, handle)| (vector, handle))
                    }
                    &mut Self::Intx(ref mut iter) => iter.next().map(|handle| (0u16, handle)),
                }
            }
            fn size_hint(&self) -> (usize, Option<usize>) {
                match self {
                    &Self::Msi(ref iter) => iter.size_hint(),
                    &Self::MsiX(ref iter) => iter.size_hint(),
                    &Self::Intx(ref iter) => iter.size_hint(),
                }
            }
        }

        match self {
            &mut Self::MsiX(ref mut map) => IterMut::MsiX(map.iter_mut()),
            &mut Self::Msi(ref mut map) => IterMut::Msi(map.iter_mut()),
            &mut Self::Intx(ref mut single) => IterMut::Intx(std::iter::once(single)),
        }
    }
}

/// The way interrupts are sent. Unlike other PCI-based interfaces, like XHCI, it doesn't seem like
/// NVME supports operating with interrupts completely disabled.
pub enum InterruptMethod {
    /// Traditional level-triggered, INTx# interrupt pins.
    Intx,
    /// Message signaled interrupts
    Msi {
        msi_info: MsiInfo,
        log2_multiple_message_enabled: u8,
    },
    /// Extended message signaled interrupts
    MsiX(MappedMsixRegs),
}
impl InterruptMethod {
    fn is_intx(&self) -> bool {
        if let Self::Intx = self {
            true
        } else {
            false
        }
    }
    fn is_msi(&self) -> bool {
        if let Self::Msi { .. } = self {
            true
        } else {
            false
        }
    }
    fn is_msix(&self) -> bool {
        if let Self::MsiX(_) = self {
            true
        } else {
            false
        }
    }
}

#[repr(C, packed)]
pub struct NvmeRegs {
    /// Controller Capabilities
    cap_low: Mmio<u32>,
    cap_high: Mmio<u32>,
    /// Version
    vs: Mmio<u32>,
    /// Interrupt mask set
    intms: Mmio<u32>,
    /// Interrupt mask clear
    intmc: Mmio<u32>,
    /// Controller configuration
    cc: Mmio<u32>,
    /// Reserved
    _rsvd: Mmio<u32>,
    /// Controller status
    csts: Mmio<u32>,
    /// NVM subsystem reset
    nssr: Mmio<u32>,
    /// Admin queue attributes
    aqa: Mmio<u32>,
    /// Admin submission queue base address
    asq_low: Mmio<u32>,
    asq_high: Mmio<u32>,
    /// Admin completion queue base address
    acq_low: Mmio<u32>,
    acq_high: Mmio<u32>,
    /// Controller memory buffer location
    cmbloc: Mmio<u32>,
    /// Controller memory buffer size
    cmbsz: Mmio<u32>,
}

#[derive(Copy, Clone, Debug)]
pub struct NvmeNamespace {
    pub id: u32,
    pub blocks: u64,
    pub block_size: u64,
}

pub type CqId = u16;
pub type SqId = u16;
pub type CmdId = u16;
pub type AtomicCqId = AtomicU16;
pub type AtomicSqId = AtomicU16;
pub type AtomicCmdId = AtomicU16;
pub type Iv = u16;

pub struct Nvme {
    interrupt_method: Mutex<InterruptMethod>,
    pcid_interface: Mutex<PciFunctionHandle>,
    regs: RwLock<&'static mut NvmeRegs>,

    sq_ivs: RwLock<HashMap<SqId, Iv>>,
    cq_ivs: RwLock<HashMap<CqId, Iv>>,

    // maps interrupt vectors with the completion queues they have
    thread_ctxts: RwLock<HashMap<Iv, Arc<ReentrantMutex<ThreadCtxt>>>>,

    next_sqid: AtomicSqId,
    next_cqid: AtomicCqId,
}

pub struct ThreadCtxt {
    buffer: RefCell<Dma<[u8; 512 * 4096]>>, // 2MB of buffer
    buffer_prp: RefCell<Dma<[u64; 512]>>,   // 4KB of PRP for the buffer

    // Yes, technically NVME allows multiple submission queues to be mapped to the same completion
    // queue, but we don't use that feature.
    queues: RefCell<HashMap<u16, (NvmeCmdQueue, NvmeCompQueue)>>,
}

unsafe impl Send for Nvme {}
unsafe impl Sync for Nvme {}

/// How to handle full submission queues.
pub enum FullSqHandling {
    /// Return an error immediately prior to posting the command.
    ErrorDirectly,

    /// Tell the executor that we want to be notified when a command on the same submission queue
    /// has been completed.
    Wait,
}

impl Nvme {
    pub fn new(
        address: usize,
        interrupt_method: InterruptMethod,
        pcid_interface: PciFunctionHandle,
    ) -> Result<Self> {
        Ok(Nvme {
            regs: RwLock::new(unsafe { &mut *(address as *mut NvmeRegs) }),
            thread_ctxts: RwLock::new(
                iter::once((
                    0_u16,
                    Arc::new(ReentrantMutex::new(ThreadCtxt {
                        buffer: RefCell::new(unsafe { Dma::zeroed()?.assume_init() }),
                        buffer_prp: RefCell::new(unsafe { Dma::zeroed()?.assume_init() }),

                        queues: RefCell::new(
                            iter::once((0, (NvmeCmdQueue::new()?, NvmeCompQueue::new()?)))
                                .collect(),
                        ),
                    })),
                ))
                .collect(),
            ),

            cq_ivs: RwLock::new(iter::once((0, 0)).collect()),
            sq_ivs: RwLock::new(iter::once((0, 0)).collect()),

            interrupt_method: Mutex::new(interrupt_method),
            pcid_interface: Mutex::new(pcid_interface),

            // TODO
            next_sqid: AtomicSqId::new(2),
            next_cqid: AtomicCqId::new(2),
        })
    }
    /// Write to a doorbell register.
    ///
    /// # Locking
    /// Locks `regs`.
    unsafe fn doorbell_write(&self, index: usize, value: u32) {
        use std::ops::DerefMut;

        let mut regs_guard = self.regs.write();
        let regs: &mut NvmeRegs = regs_guard.deref_mut();

        let dstrd = (regs.cap_high.read() & 0b1111) as usize;
        let addr = (regs as *mut NvmeRegs as usize) + 0x1000 + index * (4 << dstrd);
        (&mut *(addr as *mut Mmio<u32>)).write(value);
    }
    fn cur_thread_ctxt(&self) -> Arc<ReentrantMutex<ThreadCtxt>> {
        // TODO: multi-threading
        Arc::clone(self.thread_ctxts.read().get(&0).unwrap())
    }

    pub unsafe fn submission_queue_tail(&self, qid: u16, tail: u16) {
        self.doorbell_write(2 * (qid as usize), u32::from(tail));
    }

    pub unsafe fn completion_queue_head(&self, qid: u16, head: u16) {
        self.doorbell_write(2 * (qid as usize) + 1, u32::from(head));
    }

    pub unsafe fn init(&mut self) {
        let thread_ctxts = self.thread_ctxts.get_mut();
        {
            let regs = self.regs.read();
            log::debug!("CAP_LOW: {:X}", regs.cap_low.read());
            log::debug!("CAP_HIGH: {:X}", regs.cap_high.read());
            log::debug!("VS: {:X}", regs.vs.read());
            log::debug!("CC: {:X}", regs.cc.read());
            log::debug!("CSTS: {:X}", regs.csts.read());
        }

        log::debug!("Disabling controller.");
        self.regs.get_mut().cc.writef(1, false);

        log::trace!("Waiting for not ready.");
        loop {
            let csts = self.regs.get_mut().csts.read();
            log::trace!("CSTS: {:X}", csts);
            if csts & 1 == 1 {
                std::hint::spin_loop();
            } else {
                break;
            }
        }

        match self.interrupt_method.get_mut() {
            &mut InterruptMethod::Intx | InterruptMethod::Msi { .. } => {
                self.regs.get_mut().intms.write(0xFFFF_FFFF);
                self.regs.get_mut().intmc.write(0x0000_0001);
            }
            &mut InterruptMethod::MsiX(ref mut cfg) => {
                cfg.table_entry_pointer(0).unmask();
            }
        }

        for (qid, iv) in self.cq_ivs.get_mut().iter_mut() {
            let ctxt = thread_ctxts.get(&0).unwrap().lock();
            let queues = ctxt.queues.borrow();

            let &(ref cq, ref sq) = queues.get(qid).unwrap();
            log::debug!(
                "iv {iv} [cq {qid}: {:X}, {}] [sq {qid}: {:X}, {}]",
                cq.data.physical(),
                cq.data.len(),
                sq.data.physical(),
                sq.data.len()
            );
        }

        {
            let main_ctxt = thread_ctxts.get(&0).unwrap().lock();

            for (i, prp) in main_ctxt.buffer_prp.borrow_mut().iter_mut().enumerate() {
                *prp = (main_ctxt.buffer.borrow_mut().physical() + i * 4096) as u64;
            }

            let regs = self.regs.get_mut();

            let mut queues = main_ctxt.queues.borrow_mut();
            let (asq, acq) = queues.get_mut(&0).unwrap();
            regs.aqa
                .write(((acq.data.len() as u32 - 1) << 16) | (asq.data.len() as u32 - 1));
            regs.asq_low.write(asq.data.physical() as u32);
            regs.asq_high
                .write((asq.data.physical() as u64 >> 32) as u32);
            regs.acq_low.write(acq.data.physical() as u32);
            regs.acq_high
                .write((acq.data.physical() as u64 >> 32) as u32);

            // Set IOCQES, IOSQES, AMS, MPS, and CSS
            let mut cc = regs.cc.read();
            cc &= 0xFF00000F;
            cc |= (4 << 20) | (6 << 16);
            regs.cc.write(cc);
        }

        log::debug!("Enabling controller.");
        self.regs.get_mut().cc.writef(1, true);

        log::debug!("Waiting for ready");
        loop {
            let csts = self.regs.get_mut().csts.read();
            log::debug!("CSTS: {:X}", csts);
            if csts & 1 == 0 {
                std::hint::spin_loop();
            } else {
                break;
            }
        }
    }

    /// Masks or unmasks multiple vectors.
    ///
    /// # Panics
    /// Will panic if the same vector is called twice with different mask flags.
    pub fn set_vectors_masked(&self, vectors: impl IntoIterator<Item = (u16, bool)>) {
        let mut interrupt_method_guard = self.interrupt_method.lock();

        match &mut *interrupt_method_guard {
            &mut InterruptMethod::Intx => {
                let mut iter = vectors.into_iter();
                let (vector, mask) = match iter.next() {
                    Some(f) => f,
                    None => return,
                };
                assert_eq!(
                    iter.next(),
                    None,
                    "nvmed: internal error: multiple vectors on INTx#"
                );
                assert_eq!(vector, 0, "nvmed: internal error: nonzero vector on INTx#");
                if mask {
                    self.regs.write().intms.write(0x0000_0001);
                } else {
                    self.regs.write().intmc.write(0x0000_0001);
                }
            }
            &mut InterruptMethod::Msi {
                msi_info: _,
                log2_multiple_message_enabled: log2_enabled_messages,
            } => {
                let mut to_mask = 0x0000_0000;
                let mut to_clear = 0x0000_0000;

                for (vector, mask) in vectors {
                    assert!(
                        vector < (1 << log2_enabled_messages),
                        "nvmed: internal error: MSI vector out of range"
                    );
                    let vector = vector as u8;

                    if mask {
                        assert_ne!(
                            to_clear & (1 << vector),
                            (1 << vector),
                            "nvmed: internal error: cannot both mask and set"
                        );
                        to_mask |= 1 << vector;
                    } else {
                        assert_ne!(
                            to_mask & (1 << vector),
                            (1 << vector),
                            "nvmed: internal error: cannot both mask and set"
                        );
                        to_clear |= 1 << vector;
                    }
                }

                if to_mask != 0 {
                    self.regs.write().intms.write(to_mask);
                }
                if to_clear != 0 {
                    self.regs.write().intmc.write(to_clear);
                }
            }
            &mut InterruptMethod::MsiX(ref mut cfg) => {
                for (vector, mask) in vectors {
                    cfg.table_entry_pointer(vector.into()).set_masked(mask);
                }
            }
        }
    }
    pub fn set_vector_masked(&self, vector: u16, masked: bool) {
        self.set_vectors_masked(std::iter::once((vector, masked)))
    }

    pub async fn submit_and_complete_command(
        &self,
        sq_id: SqId,
        cmd_init: impl FnOnce(CmdId) -> NvmeCmd,
    ) -> NvmeComp {
        NvmeExecutor::current().submit(sq_id, cmd_init(0)).await
    }

    pub async fn submit_and_complete_admin_command(
        &self,
        cmd_init: impl FnOnce(CmdId) -> NvmeCmd,
    ) -> NvmeComp {
        self.submit_and_complete_command(0, cmd_init).await
    }
    pub fn try_submit_raw(
        &self,
        ctxt: &ThreadCtxt,
        sq_id: SqId,
        cmd_init: impl FnOnce(CmdId) -> NvmeCmd,
        fail: impl FnOnce(),
    ) -> Option<(CqId, CmdId)> {
        match ctxt.queues.borrow_mut().get_mut(&sq_id).unwrap() {
            (sq, _cq) => {
                if sq.is_full() {
                    fail();
                    return None;
                }
                let cmd_id = sq.tail;
                let tail = sq.submit_unchecked(cmd_init(cmd_id));

                // TODO: Submit in bulk
                unsafe {
                    self.submission_queue_tail(sq_id, tail);
                }
                Some((sq_id, cmd_id))
            }
        }
    }

    pub async fn create_io_completion_queue(
        &self,
        io_cq_id: CqId,
        vector: Option<Iv>,
    ) -> NvmeCompQueue {
        let queue = NvmeCompQueue::new().expect("nvmed: failed to allocate I/O completion queue");

        let len = u16::try_from(queue.data.len())
            .expect("nvmed: internal error: I/O CQ longer than 2^16 entries");
        let raw_len = len
            .checked_sub(1)
            .expect("nvmed: internal error: CQID 0 for I/O CQ");

        let comp = self
            .submit_and_complete_admin_command(|cid| {
                NvmeCmd::create_io_completion_queue(
                    cid,
                    io_cq_id,
                    queue.data.physical(),
                    raw_len,
                    vector,
                )
            })
            .await;

        /*match comp.status.specific {
            1 => panic!("invalid queue identifier"),
            2 => panic!("invalid queue size"),
            8 => panic!("invalid interrupt vector"),
            _ => (),
        }*/

        queue
    }
    pub async fn create_io_submission_queue(&self, io_sq_id: SqId, io_cq_id: CqId) -> NvmeCmdQueue {
        let q = NvmeCmdQueue::new().expect("failed to create submission queue");

        let len = u16::try_from(q.data.len())
            .expect("nvmed: internal error: I/O SQ longer than 2^16 entries");
        let raw_len = len
            .checked_sub(1)
            .expect("nvmed: internal error: SQID 0 for I/O SQ");

        let comp = self
            .submit_and_complete_admin_command(|cid| {
                NvmeCmd::create_io_submission_queue(
                    cid,
                    io_sq_id,
                    q.data.physical(),
                    raw_len,
                    io_cq_id,
                )
            })
            .await;
        /*match comp.status.specific {
            0 => panic!("completion queue invalid"),
            1 => panic!("invalid queue identifier"),
            2 => panic!("invalid queue size"),
            _ => (),
        }*/

        q
    }

    pub async fn init_with_queues(&self) -> BTreeMap<u32, NvmeNamespace> {
        log::trace!("preinit");

        self.identify_controller().await;

        let nsids = self.identify_namespace_list(0).await;

        log::debug!("first commands");

        let mut namespaces = BTreeMap::new();

        for nsid in nsids.iter().copied() {
            namespaces.insert(nsid, self.identify_namespace(nsid).await);
        }

        // TODO: Multiple queues
        let cq = self.create_io_completion_queue(1, Some(0)).await;
        log::trace!("created compq");
        let sq = self.create_io_submission_queue(1, 1).await;
        log::trace!("created subq");
        self.thread_ctxts
            .read()
            .get(&0)
            .unwrap()
            .lock()
            .queues
            .borrow_mut()
            .insert(1, (sq, cq));
        self.sq_ivs.write().insert(1, 0);
        self.cq_ivs.write().insert(1, 0);

        namespaces
    }

    async fn namespace_rw(
        &self,
        ctxt: &ThreadCtxt,
        namespace: &NvmeNamespace,
        lba: u64,
        blocks_1: u16,
        write: bool,
    ) -> Result<()> {
        let block_size = namespace.block_size;

        let prp = ctxt.buffer_prp.borrow_mut();
        let bytes = ((blocks_1 as u64) + 1) * block_size;
        let (ptr0, ptr1) = if bytes <= 4096 {
            (prp[0], 0)
        } else if bytes <= 8192 {
            (prp[0], prp[1])
        } else {
            (prp[0], (prp.physical() + 8) as u64)
        };

        let mut cmd = NvmeCmd::default();
        let comp = self
            .submit_and_complete_command(1, |cid| {
                cmd = if write {
                    NvmeCmd::io_write(cid, namespace.id, lba, blocks_1, ptr0, ptr1)
                } else {
                    NvmeCmd::io_read(cid, namespace.id, lba, blocks_1, ptr0, ptr1)
                };
                cmd.clone()
            })
            .await;

        let status = comp.status >> 1;
        if status == 0 {
            Ok(())
        } else {
            log::error!("command {:#x?} failed with status {:#x}", cmd, status);
            Err(Error::new(EIO))
        }
    }

    pub async fn namespace_read(
        &self,
        namespace: &NvmeNamespace,
        mut lba: u64,
        buf: &mut [u8],
    ) -> Result<usize> {
        let ctxt = self.cur_thread_ctxt();
        let ctxt = ctxt.lock();

        let block_size = namespace.block_size as usize;

        for chunk in buf.chunks_mut(/* TODO: buf len */ 8192) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            self.namespace_rw(&*ctxt, namespace, lba, (blocks - 1) as u16, false)
                .await?;

            chunk.copy_from_slice(&ctxt.buffer.borrow()[..chunk.len()]);

            lba += blocks as u64;
        }

        Ok(buf.len())
    }

    pub async fn namespace_write(
        &self,
        namespace: &NvmeNamespace,
        mut lba: u64,
        buf: &[u8],
    ) -> Result<usize> {
        let ctxt = self.cur_thread_ctxt();
        let ctxt = ctxt.lock();

        let block_size = namespace.block_size as usize;

        for chunk in buf.chunks(/* TODO: buf len */ 8192) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            ctxt.buffer.borrow_mut()[..chunk.len()].copy_from_slice(chunk);

            self.namespace_rw(&*ctxt, namespace, lba, (blocks - 1) as u16, true)
                .await?;

            lba += blocks as u64;
        }

        Ok(buf.len())
    }
}
