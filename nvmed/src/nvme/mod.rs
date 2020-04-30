use std::convert::TryFrom;
use std::collections::BTreeMap;
use std::fs::File;
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicUsize, AtomicU16, Ordering};
use std::ptr;

use crossbeam_channel::Sender;
use smallvec::{smallvec, SmallVec};

use syscall::io::{Dma, Io, Mmio};
use syscall::error::{Error, Result, EINVAL};

pub mod cq_reactor;
use self::cq_reactor::NotifReq;

use pcid_interface::msi::{MsiCapability, MsixCapability, MsixTableEntry};
use pcid_interface::PcidServerHandle;

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
            Msi(BTreeIterMut<'a, u16, File>),
            MsiX(BTreeIterMut<'a, u8, File>),
            Intx(Once<&'a mut File>),
        }
        impl<'a> Iterator for IterMut<'a> {
            type Item = (u16, &'a mut File);

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    &mut Self::Msi(ref mut iter) => iter.next().map(|&mut (vector, ref mut handle)| (u16::from(vector), handle)),
                    &mut Self::MsiX(ref mut iter) => iter.next(),
                    &mut Self::Intx(ref mut iter) => (0, iter.next()),
                }
            }
            fn size_hint(&self) -> (usize, Option<usize>) {
                match self {
                    &Self::Msi(mut iter) => iter.size_hint(),
                    &Self::MsiX(mut iter) => iter.size_hint(),
                    &Self::Intx(mut iter) => iter.size_hint(),
                }
            }
        }

        match self {
            &mut Self::MsiX(ref mut map) => IterMut::MsiX(map.iter_mut()),
            &mut Self::Msi(ref mut map) => IterMut::Msi(map.iter_mut()),
            &mut Self::Intx(ref mut single) => IterMut::Intx(false, single),
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
        } else { false }
    }
    fn is_msi(&self) -> bool {
        if let Self::Msi(_) = self {
            true
        } else { false }
    }
    fn is_msix(&self) -> bool {
        if let Self::MsiX(_) = self {
            true
        } else { false }
    }
}

pub struct MsixCfg {
    pub cap: MsixCapability,
    pub table: &'static mut [MsixTableEntry],
    pub pba: &'static mut [Mmio<u64>],
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(packed)]
pub struct NvmeCmd {
    /// Opcode
    opcode: u8,
    /// Flags
    flags: u8,
    /// Command ID
    cid: u16,
    /// Namespace identifier
    nsid: u32,
    /// Reserved
    _rsvd: u64,
    /// Metadata pointer
    mptr: u64,
    /// Data pointer
    dptr: [u64; 2],
    /// Command dword 10
    cdw10: u32,
    /// Command dword 11
    cdw11: u32,
    /// Command dword 12
    cdw12: u32,
    /// Command dword 13
    cdw13: u32,
    /// Command dword 14
    cdw14: u32,
    /// Command dword 15
    cdw15: u32,
}

impl NvmeCmd {
    pub fn create_io_completion_queue(cid: u16, qid: u16, ptr: usize, size: u16, iv: Option<u16>) -> Self {
        const DW11_PHYSICALLY_CONTIGUOUS_BIT: u32 = 0x0000_0001;
        const DW11_ENABLE_INTERRUPTS_BIT: u32 =     0x0000_0002;
        const DW11_INTERRUPT_VECTOR_SHIFT: u8 =     16;

        Self {
            opcode: 5,
            flags: 0,
            cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: ((size as u32) << 16) | (qid as u32),

            cdw11: DW11_PHYSICALLY_CONTIGUOUS_BIT | if let Some(iv) = iv {
                // enable interrupts if a vector is present
                DW11_ENABLE_INTERRUPTS_BIT
                    | (u32::from(iv) << DW11_INTERRUPT_VECTOR_SHIFT)
            } else { 0 },

            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn create_io_submission_queue(cid: u16, qid: u16, ptr: usize, size: u16, cqid: u16) -> Self {
        Self {
            opcode: 1,
            flags: 0,
            cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: ((size as u32) << 16) | (qid as u32),
            cdw11: ((cqid as u32) << 16) | 1 /* Physically Contiguous */, //TODO: QPRIO
            cdw12: 0, //TODO: NVMSETID
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn identify_namespace(cid: u16, ptr: usize, nsid: u32) -> Self {
        Self {
            opcode: 6,
            flags: 0,
            cid,
            nsid,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: 0,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn identify_controller(cid: u16, ptr: usize) -> Self {
        Self {
            opcode: 6,
            flags: 0,
            cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: 1,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn identify_namespace_list(cid: u16, ptr: usize, base: u32) -> Self {
        Self {
            opcode: 6,
            flags: 0,
            cid,
            nsid: base,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: 2,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
    pub fn get_features(cid: u16, ptr: usize, fid: u8) -> Self {
        Self {
            opcode: 0xA,
            dptr: [ptr as u64, 0],
            cdw10: u32::from(fid), // TODO: SEL
            .. Default::default()
        }
    }

    pub fn io_read(cid: u16, nsid: u32, lba: u64, blocks_1: u16, ptr0: u64, ptr1: u64) -> Self {
        Self {
            opcode: 2,
            flags: 1 << 6,
            cid,
            nsid,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr0, ptr1],
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: blocks_1 as u32,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn io_write(cid: u16, nsid: u32, lba: u64, blocks_1: u16, ptr0: u64, ptr1: u64) -> Self {
        Self {
            opcode: 1,
            flags: 1 << 6,
            cid,
            nsid,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr0, ptr1],
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: blocks_1 as u32,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
}

#[derive(Clone, Copy)]
#[repr(packed)]
pub struct IdentifyControllerData {
    /// PCI vendor ID, always the same as in the PCI function header.
    pub vid: u16,
    /// PCI subsystem vendor ID.
    pub ssvid: u16,
    /// ASCII
    pub serial_no: [u8; 20],
    /// ASCII
    pub model_no: [u8; 48],
    /// ASCII
    pub firmware_rev: [u8; 8],
    // TODO: Lots of fields
    pub _4k_pad: [u8; 4096 - 72],
}

#[derive(Clone, Copy, Debug)]
#[repr(packed)]
pub struct NvmeComp {
    command_specific: u32,
    _rsvd: u32,
    sq_head: u16,
    sq_id: u16,
    cid: u16,
    status: u16,
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

pub struct NvmeCmdQueue {
    data: Dma<[NvmeCmd; 64]>,
    i: usize,
}

impl NvmeCmdQueue {
    fn new() -> Result<Self> {
        Ok(Self {
            data: Dma::zeroed()?,
            i: 0,
        })
    }

    fn submit(&mut self, entry: NvmeCmd) -> usize {
        self.data[self.i] = entry;
        self.i = (self.i + 1) % self.data.len();
        self.i
    }
}

pub struct NvmeCompQueue {
    data: Dma<[NvmeComp; 256]>,
    i: usize,
    phase: bool,
}

impl NvmeCompQueue {
    fn new() -> Result<Self> {
        Ok(Self {
            data: Dma::zeroed()?,
            i: 0,
            phase: true,
        })
    }

    pub (crate) fn complete(&mut self) -> Option<(usize, NvmeComp)> {
        let entry = unsafe {
            ptr::read_volatile(self.data.as_ptr().add(self.i))
        };
        // println!("{:?}", entry);
        if ((entry.status & 1) == 1) == self.phase {
            self.i = (self.i + 1) % self.data.len();
            if self.i == 0 {
                self.phase = ! self.phase;
            }
            Some((self.i, entry))
        } else {
            None
        }
    }

    fn complete_spin(&mut self) -> (usize, NvmeComp) {
        loop {
            if let Some(some) = self.complete() {
                return some;
            } else {
                unsafe { std::arch::x86_64::_mm_pause() }
            }
        }
    }
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
    pub(crate) completion_queues: RwLock<BTreeMap<CqId, Mutex<(NvmeCompQueue, SmallVec<[SqId; 16]>)>>>,

    // maps interrupt vectors with the completion queues they have
    cqs_for_ivs: RwLock<BTreeMap<u16, SmallVec<[CqId; 4]>>>,

    buffer: Mutex<Dma<[u8; 512 * 4096]>>, // 2MB of buffer
    buffer_prp: Mutex<Dma<[u64; 512]>>, // 4KB of PRP for the buffer
    reactor_sender: Sender<cq_reactor::NotifReq>,

    next_sqid: AtomicSqId,
    next_cqid: AtomicCqId,
}
unsafe impl Send for Nvme {}
unsafe impl Sync for Nvme {}

impl Nvme {
    pub fn new(address: usize, interrupt_method: InterruptMethod, pcid_interface: PcidServerHandle, reactor_sender: Sender<NotifReq>) -> Result<Self> {
        Ok(Nvme {
            regs: Mutex::new(unsafe { &mut *(address as *mut NvmeRegs) }),
            submission_queues: RwLock::new(vec! [Mutex::new(NvmeCmdQueue::new()?), Mutex::new(NvmeCmdQueue::new()?)]),
            completion_queues: RwLock::new(vec! [Mutex::new(NvmeCompQueue::new()?), Mutex::new(NvmeCompQueue::new()?)]),
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
        let mut regs_guard = self.regs.write().unwrap();

        let dstrd = ((regs_guard.cap.read() >> 32) & 0b1111) as usize;
        let addr = ((*regs_guard) as *mut NvmeRegs as usize)
            + 0x1000
            + index * (4 << dstrd);
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
                unsafe { std::arch::x86_64::_mm_pause() }
            } else {
                break;
            }
        }

        // println!("  - Mask all interrupts");
        if !self.interrupt_method.get_mut().unwrap().is_msix() {
            self.regs.get_mut().unwrap().intms.write(0xFFFFFFFF);
            self.regs.get_mut().unwrap().intmc.write(0xFFFFFFFE);
        }

        for (qid, queue) in self.completion_queues.get_mut().unwrap().iter().enumerate() {
            let data = &queue.get_mut().unwrap().data;
            // println!("    - completion queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        for (qid, queue) in self.submission_queues.get_mut().unwrap().iter().enumerate() {
            let data = &queue.get_mut().unwrap().data;
            // println!("    - submission queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        {
            let regs = self.regs.get_mut().unwrap();
            let submission_queues = self.submission_queues.get_mut().unwrap();
            let completion_queues = self.submission_queues.get_mut().unwrap();

            let asq = &submission_queues[0].get_mut().unwrap();
            let acq = &completion_queues[0].get_mut().unwrap();
            regs.aqa.write(((acq.data.len() as u32 - 1) << 16) | (asq.data.len() as u32 - 1));
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
                unsafe { std::arch::x86_64::_mm_pause() }
            } else {
                break;
            }
        }
    }
    pub fn submit_command<F: FnOnce(CmdId) -> NvmeCmd>(&self, sq_id: SqId, f: F) -> CmdId {
        let sqs_read_guard = self.submission_queues.read().unwrap();
        let sq_lock = sqs_read_guard.get(&sq_id).expect("nvmed: internal error: given SQ for SQ ID not there").lock().unwrap();
        let cmd_id = u16::try_from(sq_lock.i).expect("nvmed: internal error: CQ has more than 2^16 entries");
        let tail = sq_lock.submit(f(cmd_id));
        self.submission_queue_tail(sq_id, tail);
        cmd_id
    }
    pub fn submit_admin_command<F: FnOnce(CmdId) -> NvmeCmd>(&self, f: F) -> CmdId {
        self.submit_admin_command(0, f)
    }
    pub async fn admin_queue_completion(&self, cmd_id: CmdId) -> NvmeComp {
        self.completion(0, cmd_id, 0).await
    }

    /// Returns the serial number, model, and firmware, in that order.
    pub async fn identify_controller(&self) {
        // TODO: Use same buffer
        let data: Dma<IdentifyControllerData> = Dma::zeroed().unwrap();

        // println!("  - Attempting to identify controller");
        let cid = self.submit_admin_command(|cid| NvmeCmd::identify_controller(
            cid, data.physical()
        ));

        // println!("  - Waiting to identify controller");
        let comp = self.admin_queue_completion(cid).await;

        // println!("  - Dumping identify controller");

        let model_cow = String::from_utf8_lossy(&data.model_no);
        let serial_cow = String::from_utf8_lossy(&data.serial_no);
        let fw_cow = String::from_utf8_lossy(&data.firmware_rev);

        let model = model_cow.trim();
        let serial = serial_cow.trim();
        let firmware = fw_cow.trim();

        println!(
            "  - Model: {} Serial: {} Firmware: {}",
            model,
            serial,
            firmware,
        );
    }
    pub async fn identify_namespace_list(&self, base: u32) -> Vec<u32> {
        // TODO: Use buffer
        let data: Dma<[u32; 1024]> = Dma::zeroed().unwrap();

        // println!("  - Attempting to retrieve namespace ID list");
        let cmd_id = self.submit_admin_command(|cid| NvmeCmd::identify_namespace_list(
            cid, data.physical(), base
        ));

        // println!("  - Waiting to retrieve namespace ID list");
        let comp = self.admin_queue_completion(cmd_id).await;

        // println!("  - Dumping namespace ID list");
        data.iter().copied().take_while(|&nsid| nsid != 0).collect()
    }
    pub async fn identify_namespace(&self, nsid: u32) -> NvmeNamespace {
        //TODO: Use buffer
        let data: Dma<[u8; 4096]> = Dma::zeroed().unwrap();

        // println!("  - Attempting to identify namespace {}", nsid);
        let cmd_id = self.submit_admin_command(|cid| NvmeCmd::identify_namespace(
            cid, data.physical(), nsid
        ));

        // println!("  - Waiting to identify namespace {}", nsid);
        let comp = self.admin_queue_completion(cmd_id).await;

        // println!("  - Dumping identify namespace");

        let size = *(data.as_ptr().offset(0) as *const u64);
        let capacity = *(data.as_ptr().offset(8) as *const u64);
        println!(
            "    - ID: {} Size: {} Capacity: {}",
            nsid,
            size,
            capacity
        );

        //TODO: Read block size

        NvmeNamespace {
            id: nsid,
            blocks: size,
            block_size: 512, // TODO
        }
    }
    pub async fn create_io_completion_queue(&self, io_cq_id: CqId, vector: Option<u16>) {
        let (ptr, len) = {
            let mut completion_queues_guard = self.completion_queues.write().unwrap();

            let queue_guard = completion_queues_guard.entry(io_cq_id).or_insert_with(|| {
                let queue = NvmeCompQueue::new().expect("nvmed: failed to allocate I/O completion queue");
                let sqs = SmallVec::new();
                Mutex::new((queue, sqs))
            }).get_mut().unwrap();

            let &(ref queue, _) = &*queue_guard;
            (queue.data.physical(), queue.data.len())
        };

        let len = u16::try_from(len).expect("nvmed: internal error: I/O CQ longer than 2^16 entries");
        let raw_len = len.checked_sub(1).expect("nvmed: internal error: CQID 0 for I/O CQ");

        let cmd_id = self.submit_admin_command(|cid| NvmeCmd::create_io_completion_queue(
            cid, io_cq_id, ptr, raw_len, vector,
        ));
        let comp = self.admin_queue_completion(cmd_id).await;

        if let Some(vector) = vector {
            self.cqs_for_ivs.write().unwrap().entry(vector).or_insert_with(SmallVec::new).push(io_cq_id);
        }
    }
    pub async fn create_io_submission_queue(&self, io_sq_id: SqId, io_cq_id: CqId) {
        let (ptr, len) = {
            let mut submission_queues_guard = self.submission_queues.write().unwrap();

            let queue_guard = submission_queues_guard.entry(io_sq_id).or_insert_with(|| {
                Mutex::new(NvmeCmdQueue::new().expect("nvmed: failed to allocate I/O completion queue"))
            }).get_mut().unwrap();
            (queue_guard.data.physical(), queue_guard.data.len())
        };

        let len = u16::try_from(len).expect("nvmed: internal error: I/O SQ longer than 2^16 entries");
        let raw_len = len.checked_sub(1).expect("nvmed: internal error: SQID 0 for I/O SQ");

        let cmd_id = self.submit_admin_command(|cid| NvmeCmd::create_io_submission_queue(
            cid, io_sq_id, ptr, raw_len, io_cq_id,
        ));
        let comp = self.admin_queue_completion(cmd_id).await;
    }

    pub async fn init_with_queues(&self) -> BTreeMap<u32, NvmeNamespace> {
        let ((), nsids) = futures::join!(self.identify_controller(), self.identify_namespace_list(0));

        let mut namespaces = BTreeMap::new();

        for nsid in nsids.iter().copied() {
            namespaces.insert(nsid, self.identify_namespace(nsid).await);
        }

        // TODO: Multiple queues
        self.create_io_completion_queue(1, Some(0)).await;
        self.create_io_submission_queue(1, 1).await;

        namespaces
    }

    unsafe fn namespace_rw(&mut self, nsid: u32, lba: u64, blocks_1: u16, write: bool) -> Result<()> {
        //TODO: Get real block size
        let block_size = 512;

        let bytes = ((blocks_1 as u64) + 1) * block_size;
        let (ptr0, ptr1) = if bytes <= 4096 {
            (self.buffer_prp[0], 0)
        } else if bytes <= 8192 {
            (self.buffer_prp[0], self.buffer_prp[1])
        } else {
            (self.buffer_prp[0], (self.buffer_prp.physical() + 8) as u64)
        };

        {
            let qid = 1;
            let queue = &mut self.submission_queues[qid];
            let cid = queue.i as u16;
            let entry = if write {
                NvmeCmd::io_write(
                    cid, nsid, lba, blocks_1, ptr0, ptr1
                )
            } else {
                NvmeCmd::io_read(
                    cid, nsid, lba, blocks_1, ptr0, ptr1
                )
            };
            let tail = queue.submit(entry);
            self.submission_queue_tail(qid as u16, tail as u16);
        }

        {
            let qid = 1;
            let queue = &mut self.completion_queues[qid];
            let (head, entry) = queue.complete_spin();
            //TODO: Handle errors
            self.completion_queue_head(qid as u16, head as u16);
        }

        Ok(())
    }

    pub unsafe fn namespace_read(&mut self, nsid: u32, mut lba: u64, buf: &mut [u8]) -> Result<Option<usize>> {
        //TODO: Use interrupts

        //TODO: Get real block size
        let block_size = 512;

        for chunk in buf.chunks_mut(self.buffer.len()) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            self.namespace_rw(nsid, lba, (blocks - 1) as u16, false)?;

            chunk.copy_from_slice(&self.buffer[..chunk.len()]);

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }

    pub unsafe fn namespace_write(&mut self, nsid: u32, mut lba: u64, buf: &[u8]) -> Result<Option<usize>> {
        //TODO: Use interrupts

        //TODO: Get real block size
        let block_size = 512;

        for chunk in buf.chunks(self.buffer.len()) {
            let blocks = (chunk.len() + block_size - 1) / block_size;

            assert!(blocks > 0);
            assert!(blocks <= 0x1_0000);

            self.buffer[..chunk.len()].copy_from_slice(chunk);

            self.namespace_rw(nsid, lba, (blocks - 1) as u16, true)?;

            lba += blocks as u64;
        }

        Ok(Some(buf.len()))
    }
}
