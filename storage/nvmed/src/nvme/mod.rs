use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fs::File;
use std::ptr;
use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};
use std::thread;

use crossbeam_channel::Sender;
use smallvec::{smallvec, SmallVec};

use common::io::{Io, Mmio};
use syscall::error::{Error, Result, EINVAL, EIO};

use common::dma::Dma;

pub mod cmd;
pub mod cq_reactor;
pub mod identify;
pub mod queues;

use self::cq_reactor::NotifReq;
pub use self::queues::{NvmeCmd, NvmeCmdQueue, NvmeComp, NvmeCompQueue};

use pcid_interface::msi::{MsiInfo, MsixInfo, MsixTableEntry};
use pcid_interface::PciFunctionHandle;

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) unsafe fn pause() {
    std::arch::aarch64::__yield();
}

#[cfg(target_arch = "x86")]
#[inline(always)]
pub(crate) unsafe fn pause() {
    std::arch::x86::_mm_pause();
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) unsafe fn pause() {
    std::arch::x86_64::_mm_pause();
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub(crate) unsafe fn pause() {
    std::arch::riscv64::pause();
}

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

pub struct MappedMsixRegs {
    pub info: MsixInfo,
    pub table: &'static mut [MsixTableEntry],
}

#[repr(packed)]
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
    pcid_interface: Mutex<PciFunctionHandle>,
    regs: RwLock<&'static mut NvmeRegs>,

    pub(crate) submission_queues: RwLock<BTreeMap<SqId, (Mutex<NvmeCmdQueue>, CqId)>>,
    pub(crate) completion_queues:
        RwLock<BTreeMap<CqId, Mutex<(NvmeCompQueue, SmallVec<[SqId; 16]>)>>>,

    // maps interrupt vectors with the completion queues they have
    cqs_for_ivs: RwLock<BTreeMap<u16, SmallVec<[CqId; 4]>>>,

    buffer: Mutex<Dma<[u8; 512 * 4096]>>, // 2MB of buffer
    buffer_prp: Mutex<Dma<[u64; 512]>>,   // 4KB of PRP for the buffer
    reactor_sender: Sender<cq_reactor::NotifReq>,

    next_sqid: AtomicSqId,
    next_cqid: AtomicCqId,

    next_avail_submission_epoch: AtomicU64,
}
unsafe impl Send for Nvme {}
unsafe impl Sync for Nvme {}

/// How to handle full submission queues.
pub enum FullSqHandling {
    /// Return an error immediately prior to posting the command.
    ErrorDirectly,

    /// Tell the IRQ reactor that we want to be notified when a command on the same submission
    /// queue has been completed.
    Wait,
}

impl Nvme {
    pub fn new(
        address: usize,
        interrupt_method: InterruptMethod,
        pcid_interface: PciFunctionHandle,
        reactor_sender: Sender<NotifReq>,
    ) -> Result<Self> {
        Ok(Nvme {
            regs: RwLock::new(unsafe { &mut *(address as *mut NvmeRegs) }),
            submission_queues: RwLock::new(
                std::iter::once((0u16, (Mutex::new(NvmeCmdQueue::new()?), 0u16))).collect(),
            ),
            completion_queues: RwLock::new(
                std::iter::once((0u16, Mutex::new((NvmeCompQueue::new()?, smallvec!(0)))))
                    .collect(),
            ),
            // map the zero interrupt vector (which according to the spec shall always point to the
            // admin completion queue) to CQID 0 (admin completion queue)
            cqs_for_ivs: RwLock::new(std::iter::once((0, smallvec!(0))).collect()),
            buffer: Mutex::new(unsafe { Dma::zeroed()?.assume_init() }),
            buffer_prp: Mutex::new(unsafe { Dma::zeroed()?.assume_init() }),
            interrupt_method: Mutex::new(interrupt_method),
            pcid_interface: Mutex::new(pcid_interface),
            reactor_sender,

            next_sqid: AtomicSqId::new(0),
            next_cqid: AtomicCqId::new(0),
            next_avail_submission_epoch: AtomicU64::new(0),
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

        let dstrd = (regs.cap_high.read() & 0b1111) as usize;
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

        {
            let regs = self.regs.read().unwrap();
            log::debug!("CAP_LOW: {:X}", regs.cap_low.read());
            log::debug!("CAP_HIGH: {:X}", regs.cap_high.read());
            log::debug!("VS: {:X}", regs.vs.read());
            log::debug!("CC: {:X}", regs.cc.read());
            log::debug!("CSTS: {:X}", regs.csts.read());
        }

        log::debug!("Disabling controller.");
        self.regs.get_mut().unwrap().cc.writef(1, false);

        log::trace!("Waiting for not ready.");
        loop {
            let csts = self.regs.get_mut().unwrap().csts.read();
            log::trace!("CSTS: {:X}", csts);
            if csts & 1 == 1 {
                pause();
            } else {
                break;
            }
        }

        match self.interrupt_method.get_mut().unwrap() {
            &mut InterruptMethod::Intx | InterruptMethod::Msi { .. } => {
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
            log::debug!(
                "completion queue {}: {:X}, {}, (submission queue ids: {:?}",
                qid,
                data.physical(),
                data.len(),
                sq_ids
            );
        }

        for (qid, (queue, cq_id)) in self.submission_queues.get_mut().unwrap().iter_mut() {
            let data = &queue.get_mut().unwrap().data;
            log::debug!(
                "submission queue {}: {:X}, {}, attached to CQID: {}",
                qid,
                data.physical(),
                data.len(),
                cq_id
            );
        }

        {
            let regs = self.regs.get_mut().unwrap();
            let submission_queues = self.submission_queues.get_mut().unwrap();
            let completion_queues = self.completion_queues.get_mut().unwrap();

            let asq = submission_queues.get_mut(&0).unwrap().0.get_mut().unwrap();
            let (acq, _) = completion_queues.get_mut(&0).unwrap().get_mut().unwrap();
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
        self.regs.get_mut().unwrap().cc.writef(1, true);

        log::debug!("Waiting for ready");
        loop {
            let csts = self.regs.get_mut().unwrap().csts.read();
            log::debug!("CSTS: {:X}", csts);
            if csts & 1 == 0 {
                pause();
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

    #[cfg(not(feature = "async"))]
    pub fn submit_and_complete_command<F: FnOnce(CmdId) -> NvmeCmd>(
        &self,
        sq_id: SqId,
        cmd_init: F,
    ) -> NvmeComp {
        // Submit command
        let cmd = {
            let sqs_read_guard = self.submission_queues.read().unwrap();
            let &(ref sq_lock, cq_id) = sqs_read_guard
                .get(&sq_id)
                .expect("nvmed: internal error: given SQ for SQ ID not there");
            let mut sq_guard = sq_lock.lock().unwrap();
            let sq = &mut *sq_guard;

            assert!(!sq.is_full());

            let cmd_id = u16::try_from(sq.tail)
                .expect("nvmed: internal error: CQ has more than 2^16 entries");
            let cmd = cmd_init(cmd_id);
            log::trace!(
                "Sent submission queue entry (SQID {}): {:?} at {}",
                sq_id,
                cmd,
                cmd_id
            );
            let tail = sq.submit_unchecked(cmd);
            let tail = u16::try_from(tail).unwrap();

            // make sure that we register interest before the reactor can get notified
            unsafe { self.submission_queue_tail(sq_id, tail) };

            cmd
        };

        // Read completion
        loop {
            for (cq_id, completion_queue_lock) in self.completion_queues.read().unwrap().iter() {
                if *cq_id != sq_id {
                    // Currently, CQ and SQ IDs have to match
                    continue;
                }

                let mut completion_queue_guard = completion_queue_lock.lock().unwrap();
                let &mut (ref mut completion_queue, _) = &mut *completion_queue_guard;

                while let Some((head, entry)) = completion_queue.complete(Some((sq_id, cmd))) {
                    unsafe { self.completion_queue_head(*cq_id, head) };

                    log::trace!(
                        "Got completion queue entry (CQID {}): {:?} at {}",
                        cq_id,
                        entry,
                        head
                    );

                    assert_eq!(sq_id, { entry.sq_id });
                    assert_eq!({ cmd.cid }, { entry.cid });

                    {
                        let submission_queues_read_lock = self.submission_queues.read().unwrap();
                        // this lock is actually important, since it will block during submission from other
                        // threads. the lock won't be held for long by the submitters, but it still prevents
                        // the entry being lost before this reactor is actually able to respond:
                        let &(ref sq_lock, corresponding_cq_id) = submission_queues_read_lock.get(&{entry.sq_id}).expect("nvmed: internal error: queue returned from controller doesn't exist");
                        assert_eq!(*cq_id, corresponding_cq_id);
                        let mut sq_guard = sq_lock.lock().unwrap();
                        sq_guard.head = entry.sq_head;
                    }

                    return entry;
                }
            }
            thread::yield_now();
        }
    }

    #[cfg(feature = "async")]
    pub fn submit_and_complete_command<F: FnOnce(CmdId) -> NvmeCmd>(
        &self,
        sq_id: SqId,
        cmd_init: F,
    ) -> NvmeComp {
        use crate::nvme::cq_reactor::{CompletionFuture, CompletionFutureState};
        futures::executor::block_on(CompletionFuture {
            state: CompletionFutureState::PendingSubmission {
                cmd_init,
                nvme: &self,
                sq_id,
            },
        })
    }

    pub fn submit_and_complete_admin_command<F: FnOnce(CmdId) -> NvmeCmd>(
        &self,
        cmd_init: F,
    ) -> NvmeComp {
        self.submit_and_complete_command(0, cmd_init)
    }

    pub fn create_io_completion_queue(&self, io_cq_id: CqId, vector: Option<u16>) {
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

        let comp = self.submit_and_complete_admin_command(|cid| {
            NvmeCmd::create_io_completion_queue(cid, io_cq_id, ptr, raw_len, vector)
        });

        if let Some(vector) = vector {
            self.cqs_for_ivs
                .write()
                .unwrap()
                .entry(vector)
                .or_insert_with(SmallVec::new)
                .push(io_cq_id);
        }
    }
    pub fn create_io_submission_queue(&self, io_sq_id: SqId, io_cq_id: CqId) {
        let (ptr, len) = {
            let mut submission_queues_guard = self.submission_queues.write().unwrap();

            let (queue_lock, _) = submission_queues_guard.entry(io_sq_id).or_insert_with(|| {
                (
                    Mutex::new(
                        NvmeCmdQueue::new()
                            .expect("nvmed: failed to allocate I/O completion queue"),
                    ),
                    io_cq_id,
                )
            });
            let queue = queue_lock.get_mut().unwrap();

            (queue.data.physical(), queue.data.len())
        };

        let len =
            u16::try_from(len).expect("nvmed: internal error: I/O SQ longer than 2^16 entries");
        let raw_len = len
            .checked_sub(1)
            .expect("nvmed: internal error: SQID 0 for I/O SQ");

        let comp = self.submit_and_complete_admin_command(|cid| {
            NvmeCmd::create_io_submission_queue(cid, io_sq_id, ptr, raw_len, io_cq_id)
        });
    }

    pub fn init_with_queues(&self) -> BTreeMap<u32, NvmeNamespace> {
        log::trace!("preinit");

        self.identify_controller();
        let nsids = self.identify_namespace_list(0);

        log::debug!("first commands");

        let mut namespaces = BTreeMap::new();

        for nsid in nsids.iter().copied() {
            namespaces.insert(nsid, self.identify_namespace(nsid));
        }

        // TODO: Multiple queues
        self.create_io_completion_queue(1, Some(0));
        self.create_io_submission_queue(1, 1);

        namespaces
    }

    fn namespace_rw(
        &self,
        namespace: &NvmeNamespace,
        nsid: u32,
        lba: u64,
        blocks_1: u16,
        write: bool,
    ) -> Result<()> {
        let block_size = namespace.block_size;

        let buffer_prp_guard = self.buffer_prp.lock().unwrap();

        let bytes = ((blocks_1 as u64) + 1) * block_size;
        let (ptr0, ptr1) = if bytes <= 4096 {
            (buffer_prp_guard[0], 0)
        } else if bytes <= 8192 {
            (buffer_prp_guard[0], buffer_prp_guard[1])
        } else {
            (
                buffer_prp_guard[0],
                (buffer_prp_guard.physical() + 8) as u64,
            )
        };

        let mut cmd = NvmeCmd::default();
        let comp = self.submit_and_complete_command(1, |cid| {
            cmd = if write {
                NvmeCmd::io_write(cid, nsid, lba, blocks_1, ptr0, ptr1)
            } else {
                NvmeCmd::io_read(cid, nsid, lba, blocks_1, ptr0, ptr1)
            };
            cmd.clone()
        });
        let status = comp.status >> 1;
        if status == 0 {
            Ok(())
        } else {
            log::error!("command {:#x?} failed with status {:#x}", cmd, status);
            Err(Error::new(EIO))
        }
    }

    pub fn namespace_read(
        &self,
        namespace: &NvmeNamespace,
        nsid: u32,
        mut lba: u64,
        buf: &mut [u8],
    ) -> Result<Option<usize>> {
        let block_size = namespace.block_size as usize;

        let buffer_guard = self.buffer.lock().unwrap();

        for chunk in buf.chunks_mut(/*TODO: buffer_guard.len()*/ 8192) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            self.namespace_rw(namespace, nsid, lba, (blocks - 1) as u16, false)?;

            chunk.copy_from_slice(&buffer_guard[..chunk.len()]);

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }

    pub fn namespace_write(
        &self,
        namespace: &NvmeNamespace,
        nsid: u32,
        mut lba: u64,
        buf: &[u8],
    ) -> Result<Option<usize>> {
        let block_size = namespace.block_size as usize;

        let mut buffer_guard = self.buffer.lock().unwrap();

        for chunk in buf.chunks(/*TODO: buffer_guard.len()*/ 8192) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            buffer_guard[..chunk.len()].copy_from_slice(chunk);

            self.namespace_rw(namespace, nsid, lba, (blocks - 1) as u16, true)?;

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }
}
