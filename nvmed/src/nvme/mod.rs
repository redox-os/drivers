use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fs::File;
use std::ptr;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};

use crossbeam_channel::Sender;
use smallvec::{smallvec, SmallVec};

use syscall::error::{Error, Result, EINVAL};
use syscall::io::{Dma, Io, Mmio};

pub mod cmd;
pub mod cq_reactor;
pub mod identify;
pub mod queues;

use self::cq_reactor::NotifReq;
pub use self::queues::{NvmeCmd, NvmeCmdQueue, NvmeComp, NvmeCompQueue};

use pcid_interface::msi::{MsiCapability, MsixCapability, MsixTableEntry};
use pcid_interface::PcidServerHandle;

/// Used in conjunction with `InterruptMethod`, primarily by the CQ reactor.
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
    Msi(MsiCapability),
    /// Extended message signaled interrupts
    MsiX(MsixCfg),
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
        if let Self::Msi(_) = self {
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

pub struct MsixCfg {
    pub cap: MsixCapability,
    pub table: &'static mut [MsixTableEntry],
    pub pba: &'static mut [Mmio<u64>],
}

#[repr(packed)]
pub struct NvmeRegs {
    /// Controller Capabilities
    cap: Mmio<u64>,
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
    asq: Mmio<u64>,
    /// Admin completion queue base address
    acq: Mmio<u64>,
    /// Controller memory buffer location
    cmbloc: Mmio<u32>,
    /// Controller memory buffer size
    cmbsz: Mmio<u32>,
}

#[derive(Debug)]
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

pub struct Nvme {
    interrupt_method: Mutex<InterruptMethod>,
    pcid_interface: Mutex<PcidServerHandle>,
    regs: RwLock<&'static mut NvmeRegs>,

    pub(crate) submission_queues: RwLock<BTreeMap<SqId, Mutex<NvmeCmdQueue>>>,
    pub(crate) completion_queues:
        RwLock<BTreeMap<CqId, Mutex<(NvmeCompQueue, SmallVec<[SqId; 16]>)>>>,

    // maps interrupt vectors with the completion queues they have
    cqs_for_ivs: RwLock<BTreeMap<u16, SmallVec<[CqId; 4]>>>,

    buffer: Mutex<Dma<[u8; 512 * 4096]>>, // 2MB of buffer
    buffer_prp: Mutex<Dma<[u64; 512]>>,   // 4KB of PRP for the buffer
    reactor_sender: Sender<cq_reactor::NotifReq>,

    next_sqid: AtomicSqId,
    next_cqid: AtomicCqId,
}
unsafe impl Send for Nvme {}
unsafe impl Sync for Nvme {}

/// How to handle full submission queues.
pub enum FullSqHandling {
    /// Return an error immediately prior to posting the command.
    ErrorDirectly,

    /// Tell the IRQ reactor that we wan't to be notified when a command on the same submission
    /// queue has been completed.
    Wait,
}

pub enum SubmissionBehavior<'a, F: FnOnce(CmdId) -> NvmeCmd> {
    Nonblocking(Result<CmdId, F>), // TODO: Add full error
    Future(self::cq_reactor::SubmissionFuture<'a, F>),
}

impl Nvme {
    pub fn new(
        address: usize,
        interrupt_method: InterruptMethod,
        pcid_interface: PcidServerHandle,
        reactor_sender: Sender<NotifReq>,
    ) -> Result<Self> {
        Ok(Nvme {
            regs: RwLock::new(unsafe { &mut *(address as *mut NvmeRegs) }),
            submission_queues: RwLock::new(
                std::iter::once((0u16, Mutex::new(NvmeCmdQueue::new()?))).collect(),
            ),
            completion_queues: RwLock::new(
                std::iter::once((0u16, Mutex::new((NvmeCompQueue::new()?, smallvec!())))).collect(),
            ),
            // map the zero interrupt vector (which according to the spec shall always point to the
            // admin completion queue) to CQID 0 (admin completion queue)
            cqs_for_ivs: RwLock::new(std::iter::once((0, smallvec!(0))).collect()),
            buffer: Mutex::new(Dma::zeroed()?),
            buffer_prp: Mutex::new(Dma::zeroed()?),
            interrupt_method: Mutex::new(interrupt_method),
            pcid_interface: Mutex::new(pcid_interface),
            reactor_sender,

            next_sqid: AtomicSqId::new(0),
            next_cqid: AtomicCqId::new(0),
        })
    }
    /// Write to a doorbell register.
    ///
    /// # Locking
    /// Locks `regs`.
    unsafe fn doorbell_write(&self, index: usize, value: u32) {
        use std::ops::DerefMut;

        let mut regs_guard = self.regs.write().unwrap();
        let mut regs: &mut NvmeRegs = regs_guard.deref_mut();

        let dstrd = ((regs.cap.read() >> 32) & 0b1111) as usize;
        let addr = (regs as *mut NvmeRegs as usize) + 0x1000 + index * (4 << dstrd);
        (&mut *(addr as *mut Mmio<u32>)).write(value);
    }

    pub unsafe fn submission_queue_tail(&self, qid: u16, tail: u16) {
        self.doorbell_write(2 * (qid as usize), u32::from(tail));
    }

    pub unsafe fn completion_queue_head(&self, qid: u16, head: u16) {
        self.doorbell_write(2 * (qid as usize) + 1, u32::from(head));
    }

    pub unsafe fn init(&mut self) {
        let mut buffer = self.buffer.get_mut().unwrap();
        let mut buffer_prp = self.buffer_prp.get_mut().unwrap();

        for i in 0..buffer_prp.len() {
            buffer_prp[i] = (buffer.physical() + i * 4096) as u64;
        }

        // println!("  - CAPS: {:X}", self.regs.cap.read());
        // println!("  - VS: {:X}", self.regs.vs.read());
        // println!("  - CC: {:X}", self.regs.cc.read());
        // println!("  - CSTS: {:X}", self.regs.csts.read());

        // println!("  - Disable");
        self.regs.get_mut().unwrap().cc.writef(1, false);

        // println!("  - Waiting for not ready");
        loop {
            let csts = self.regs.get_mut().unwrap().csts.read();
            // println!("CSTS: {:X}", csts);
            if csts & 1 == 1 {
                std::arch::x86_64::_mm_pause();
            } else {
                break;
            }
        }

        match self.interrupt_method.get_mut().unwrap() {
            &mut InterruptMethod::Intx | InterruptMethod::Msi(_) => {
                self.regs.get_mut().unwrap().intms.write(0xFFFF_FFFF);
                self.regs.get_mut().unwrap().intmc.write(0x0000_0001);
            }
            &mut InterruptMethod::MsiX(ref mut cfg) => {
                cfg.table[0].unmask();
            }
        }

        for (qid, queue) in self.completion_queues.get_mut().unwrap().iter_mut() {
            let &(ref cq, ref sq_ids) = &*queue.get_mut().unwrap();
            let data = &cq.data;
            // println!("    - completion queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        for (qid, queue) in self.submission_queues.get_mut().unwrap().iter_mut() {
            let data = &queue.get_mut().unwrap().data;
            // println!("    - submission queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        {
            let regs = self.regs.get_mut().unwrap();
            let submission_queues = self.submission_queues.get_mut().unwrap();
            let completion_queues = self.completion_queues.get_mut().unwrap();

            let asq = submission_queues.get_mut(&0).unwrap().get_mut().unwrap();
            let (acq, _) = completion_queues.get_mut(&0).unwrap().get_mut().unwrap();
            regs.aqa
                .write(((acq.data.len() as u32 - 1) << 16) | (asq.data.len() as u32 - 1));
            regs.asq.write(asq.data.physical() as u64);
            regs.acq.write(acq.data.physical() as u64);

            // Set IOCQES, IOSQES, AMS, MPS, and CSS
            let mut cc = regs.cc.read();
            cc &= 0xFF00000F;
            cc |= (4 << 20) | (6 << 16);
            regs.cc.write(cc);
        }

        // println!("  - Enable");
        self.regs.get_mut().unwrap().cc.writef(1, true);

        // println!("  - Waiting for ready");
        loop {
            let csts = self.regs.get_mut().unwrap().csts.read();
            // println!("CSTS: {:X}", csts);
            if csts & 1 == 0 {
                std::arch::x86_64::_mm_pause();
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
        let mut interrupt_method_guard = self.interrupt_method.lock().unwrap();

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
                    self.regs.write().unwrap().intms.write(0x0000_0001);
                } else {
                    self.regs.write().unwrap().intmc.write(0x0000_0001);
                }
            }
            &mut InterruptMethod::Msi(ref mut cap) => {
                let mut to_mask = 0x0000_0000;
                let mut to_clear = 0x0000_0000;

                for (vector, mask) in vectors {
                    assert!(
                        vector < (1 << cap.multi_message_enable()),
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
                    self.regs.write().unwrap().intms.write(to_mask);
                }
                if to_clear != 0 {
                    self.regs.write().unwrap().intmc.write(to_clear);
                }
            }
            &mut InterruptMethod::MsiX(ref mut cfg) => {
                for (vector, mask) in vectors {
                    cfg.table
                        .get_mut(vector as usize)
                        .expect("nvmed: internal error: MSI-X vector out of range")
                        .set_masked(mask);
                }
            }
        }
    }
    pub fn set_vector_masked(&self, vector: u16, masked: bool) {
        self.set_vectors_masked(std::iter::once((vector, masked)))
    }

    pub fn submit_command_generic<'a, F: FnOnce(CmdId) -> NvmeCmd>(&'a self, sq_id: SqId, full_sq_handling: FullSqHandling, cmd_init: F) -> SubmissionBehavior<'a, F> {
        let sqs_read_guard = self.submission_queues.read().unwrap();
        let mut sq_lock = sqs_read_guard
            .get(&sq_id)
            .expect("nvmed: internal error: given SQ for SQ ID not there")
            .lock()
            .unwrap();
        if sq_lock.is_full() {
            match full_sq_handling {
                FullSqHandling::ErrorDirectly => return SubmissionBehavior::Nonblocking(Err(cmd_init)),
                FullSqHandling::Wait => return SubmissionBehavior::Future(self.wait_for_available_submission(sq_id, cmd_init)),
            }
        }
        let cmd_id =
            u16::try_from(sq_lock.tail).expect("nvmed: internal error: CQ has more than 2^16 entries");
        let tail = sq_lock.submit_unchecked(cmd_init(cmd_id));
        let tail = u16::try_from(tail).unwrap();

        unsafe { self.submission_queue_tail(sq_id, tail) };

        match full_sq_handling {
            FullSqHandling::ErrorDirectly => SubmissionBehavior::Nonblocking(Ok(cmd_id)),
            FullSqHandling::Wait => SubmissionBehavior::Future(self::cq_reactor::SubmissionFuture { state: self::cq_reactor::SubmissionFutureState::Ready(cmd_id) })
        }
    }

    /// Try submitting a new entry to the specified submission queue, or return None if the queue
    /// was full.
    pub fn try_submit_command<F: FnOnce(CmdId) -> NvmeCmd>(
        &self,
        sq_id: SqId,
        f: F,
    ) -> Result<CmdId, F> {
        match self.submit_command_generic(sq_id, FullSqHandling::ErrorDirectly, f) {
            SubmissionBehavior::Nonblocking(opt) => opt,
            _ => unreachable!(),
        }
    }
    pub async fn submit_command_async<F: FnOnce(CmdId) -> NvmeCmd>(&self, sq_id: SqId, f: F) -> CmdId {
        match self.submit_command_generic(sq_id, FullSqHandling::Wait, f) {
            SubmissionBehavior::Future(future) => future.await,
            _ => unreachable!(),
        }
    }
    pub async fn submit_admin_command<F: FnOnce(CmdId) -> NvmeCmd>(&self, f: F) -> CmdId {
        self.submit_command_async(0, f).await
    }
    pub async fn admin_queue_completion(&self, cmd_id: CmdId) -> NvmeComp {
        self.completion(0, cmd_id, 0).await
    }

    pub async fn create_io_completion_queue(&self, io_cq_id: CqId, vector: Option<u16>) {
        let (ptr, len) = {
            let mut completion_queues_guard = self.completion_queues.write().unwrap();

            let queue_guard = completion_queues_guard
                .entry(io_cq_id)
                .or_insert_with(|| {
                    let queue = NvmeCompQueue::new()
                        .expect("nvmed: failed to allocate I/O completion queue");
                    let sqs = SmallVec::new();
                    Mutex::new((queue, sqs))
                })
                .get_mut()
                .unwrap();

            let &(ref queue, _) = &*queue_guard;
            (queue.data.physical(), queue.data.len())
        };

        let len =
            u16::try_from(len).expect("nvmed: internal error: I/O CQ longer than 2^16 entries");
        let raw_len = len
            .checked_sub(1)
            .expect("nvmed: internal error: CQID 0 for I/O CQ");

        let cmd_id = self
            .submit_admin_command(|cid| {
                NvmeCmd::create_io_completion_queue(cid, io_cq_id, ptr, raw_len, vector)
            })
            .await;
        let comp = self.admin_queue_completion(cmd_id).await;

        if let Some(vector) = vector {
            self.cqs_for_ivs
                .write()
                .unwrap()
                .entry(vector)
                .or_insert_with(SmallVec::new)
                .push(io_cq_id);
        }
    }
    pub async fn create_io_submission_queue(&self, io_sq_id: SqId, io_cq_id: CqId) {
        let (ptr, len) = {
            let mut submission_queues_guard = self.submission_queues.write().unwrap();

            let queue_guard = submission_queues_guard
                .entry(io_sq_id)
                .or_insert_with(|| {
                    Mutex::new(
                        NvmeCmdQueue::new()
                            .expect("nvmed: failed to allocate I/O completion queue"),
                    )
                })
                .get_mut()
                .unwrap();
            (queue_guard.data.physical(), queue_guard.data.len())
        };

        let len =
            u16::try_from(len).expect("nvmed: internal error: I/O SQ longer than 2^16 entries");
        let raw_len = len
            .checked_sub(1)
            .expect("nvmed: internal error: SQID 0 for I/O SQ");

        let cmd_id = self
            .submit_admin_command(|cid| {
                NvmeCmd::create_io_submission_queue(cid, io_sq_id, ptr, raw_len, io_cq_id)
            })
            .await;
        let comp = self.admin_queue_completion(cmd_id).await;
    }

    pub async fn init_with_queues(&self) -> BTreeMap<u32, NvmeNamespace> {
        let ((), nsids) =
            futures::join!(self.identify_controller(), self.identify_namespace_list(0));

        let mut namespaces = BTreeMap::new();

        for nsid in nsids.iter().copied() {
            namespaces.insert(nsid, self.identify_namespace(nsid).await);
        }

        // TODO: Multiple queues
        self.create_io_completion_queue(1, Some(0)).await;
        self.create_io_submission_queue(1, 1).await;

        namespaces
    }

    async fn namespace_rw(
        &self,
        nsid: u32,
        lba: u64,
        blocks_1: u16,
        write: bool,
    ) -> Result<()> {
        //TODO: Get real block size
        let block_size = 512;

        let buffer_prp_guard = self.buffer_prp.lock().unwrap();

        let bytes = ((blocks_1 as u64) + 1) * block_size;
        let (ptr0, ptr1) = if bytes <= 4096 {
            (buffer_prp_guard[0], 0)
        } else if bytes <= 8192 {
            (buffer_prp_guard[0], buffer_prp_guard[1])
        } else {
            (buffer_prp_guard[0], (buffer_prp_guard.physical() + 8) as u64)
        };

        let cmd_id = self.submit_command_async(1, |cid| {
            if write {
                NvmeCmd::io_write(cid, nsid, lba, blocks_1, ptr0, ptr1)
            } else {
                NvmeCmd::io_read(cid, nsid, lba, blocks_1, ptr0, ptr1)
            }
        }).await;

        let comp = self.completion(1, cmd_id, 1).await;
        // TODO: Handle errors

        Ok(())
    }

    pub async fn namespace_read(
        &self,
        nsid: u32,
        mut lba: u64,
        buf: &mut [u8],
    ) -> Result<Option<usize>> {
        //TODO: Get real block size
        let block_size = 512;

        let mut buffer_guard = self.buffer.lock().unwrap();

        for chunk in buf.chunks_mut(buffer_guard.len()) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            self.namespace_rw(nsid, lba, (blocks - 1) as u16, false).await?;

            chunk.copy_from_slice(&buffer_guard[..chunk.len()]);

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }

    pub async fn namespace_write(
        &self,
        nsid: u32,
        mut lba: u64,
        buf: &[u8],
    ) -> Result<Option<usize>> {
        //TODO: Use interrupts

        //TODO: Get real block size
        let block_size = 512;

        let mut buffer_guard = self.buffer.lock().unwrap();

        for chunk in buf.chunks(buffer_guard.len()) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            buffer_guard[..chunk.len()].copy_from_slice(chunk);

            self.namespace_rw(nsid, lba, (blocks - 1) as u16, true).await?;

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }
}
