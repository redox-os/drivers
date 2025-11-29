use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use std::iter;
use std::sync::atomic::AtomicU16;
use std::sync::Arc;

use parking_lot::{Mutex, ReentrantMutex, RwLock};
use pcid_interface::irq_helpers::InterruptVector;

use common::io::{Io, Mmio};
use common::timeout::Timeout;
use syscall::error::{Error, Result, EIO};

use common::dma::Dma;

pub mod cmd;
pub mod executor;
pub mod identify;
pub mod queues;

use self::executor::NvmeExecutor;
pub use self::queues::{NvmeCmd, NvmeCmdQueue, NvmeComp, NvmeCompQueue};

use pcid_interface::PciFunctionHandle;

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
    interrupt_vector: Mutex<InterruptVector>,
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
        interrupt_vector: InterruptVector,
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

            interrupt_vector: Mutex::new(interrupt_vector),
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

    pub unsafe fn init(&mut self) -> Result<()> {
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

        {
            log::trace!("Waiting for not ready.");
            let timeout = Timeout::from_secs(1);
            loop {
                let csts = self.regs.get_mut().csts.read();
                log::trace!("CSTS: {:X}", csts);
                if csts & 1 == 1 {
                    timeout.run().map_err(|()| {
                        log::error!("failed to wait for not ready");
                        Error::new(EIO)
                    })?;
                } else {
                    break;
                }
            }
        }

        if !self.interrupt_vector.get_mut().set_masked_if_fast(false) {
            self.regs.get_mut().intms.write(0xFFFF_FFFF);
            self.regs.get_mut().intmc.write(0x0000_0001);
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

        {
            log::debug!("Waiting for ready");
            let timeout = Timeout::from_secs(1);
            loop {
                let csts = self.regs.get_mut().csts.read();
                log::debug!("CSTS: {:X}", csts);
                if csts & 1 == 0 {
                    timeout.run().map_err(|()| {
                        log::error!("failed to wait for ready");
                        Error::new(EIO)
                    })?;
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    pub fn set_vector_masked(&self, vector: u16, masked: bool) {
        let mut interrupt_vector_guard = (&self).interrupt_vector.lock();

        if !interrupt_vector_guard.set_masked_if_fast(masked) {
            let mut to_mask = 0x0000_0000;
            let mut to_clear = 0x0000_0000;

            let vector = vector as u8;

            if masked {
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

            if to_mask != 0 {
                (&self).regs.write().intms.write(to_mask);
            }
            if to_clear != 0 {
                (&self).regs.write().intmc.write(to_clear);
            }
        }
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
