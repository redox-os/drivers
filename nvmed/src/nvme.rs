use std::thread;
use std::collections::BTreeMap;
use syscall::io::{Dma, Io, Mmio};
use syscall::error::{Error, Result, EINVAL};

#[derive(Clone, Copy)]
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
    pub fn create_io_completion_queue(cid: u16, qid: u16, ptr: usize, size: u16) -> Self {
        Self {
            opcode: 5,
            flags: 0,
            cid: cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: ((size as u32) << 16) | (qid as u32),
            cdw11: 1 /* Physically Contiguous */, //TODO: IV, IEN
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
            cid: cid,
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
            cid: cid,
            nsid: nsid,
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
            cid: cid,
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
            cid: cid,
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

    pub fn io_read(cid: u16, nsid: u32, lba: u64, blocks_1: u16, ptr0: u64, ptr1: u64) -> Self {
        Self {
            opcode: 2,
            flags: 1 << 6,
            cid: cid,
            nsid: nsid,
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
            cid: cid,
            nsid: nsid,
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

    fn complete(&mut self) -> Option<(usize, NvmeComp)> {
        let entry = self.data[self.i];
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
                thread::yield_now();
            }
        }
    }
}

pub struct NvmeNamespace {
    pub id: u32,
    pub blocks: u64,
    pub block_size: u64,
}

pub struct Nvme {
    regs: &'static mut NvmeRegs,
    submission_queues: [NvmeCmdQueue; 2],
    completion_queues: [NvmeCompQueue; 2],
    buffer: Dma<[u8; 512 * 4096]>, // 2MB of buffer
    buffer_prp: Dma<[u64; 512]>, // 4KB of PRP for the buffer
}

impl Nvme {
    pub fn new(address: usize) -> Result<Self> {
        Ok(Nvme {
            regs: unsafe { &mut *(address as *mut NvmeRegs) },
            submission_queues: [NvmeCmdQueue::new()?, NvmeCmdQueue::new()?],
            completion_queues: [NvmeCompQueue::new()?, NvmeCompQueue::new()?],
            buffer: Dma::zeroed()?,
            buffer_prp: Dma::zeroed()?,
        })
    }

    unsafe fn doorbell(&mut self, index: usize) -> &'static mut Mmio<u32> {
        let dstrd = ((self.regs.cap.read() >> 32) & 0b1111) as usize;
        let addr = (self.regs as *mut _ as usize)
            + 0x1000
            + index * (4 << dstrd);
        &mut *(addr as *mut Mmio<u32>)
    }

    pub unsafe fn submission_queue_tail(&mut self, qid: u16, tail: u16) {
        self.doorbell(2 * (qid as usize)).write(tail as u32);
    }

    pub unsafe fn completion_queue_head(&mut self, qid: u16, head: u16) {
        self.doorbell(2 * (qid as usize) + 1).write(head as u32)
    }

    pub unsafe fn init(&mut self) -> BTreeMap<u32, NvmeNamespace> {
        for i in 0..self.buffer_prp.len() {
            self.buffer_prp[i] = (self.buffer.physical() + i * 4096) as u64;
        }

        println!("  - CAPS: {:X}", self.regs.cap.read());
        println!("  - VS: {:X}", self.regs.vs.read());
        println!("  - CC: {:X}", self.regs.cc.read());
        println!("  - CSTS: {:X}", self.regs.csts.read());

        println!("  - Disable");
        self.regs.cc.writef(1, false);

        println!("  - Mask all interrupts");
        self.regs.intms.write(0xFFFFFFFF);

        for (qid, queue) in self.completion_queues.iter().enumerate() {
            let data = &queue.data;
            println!("    - completion queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        for (qid, queue) in self.submission_queues.iter().enumerate() {
            let data = &queue.data;
            println!("    - submission queue {}: {:X}, {}", qid, data.physical(), data.len());
        }

        {
            let asq = &self.submission_queues[0];
            let acq = &self.completion_queues[0];
            self.regs.aqa.write(((acq.data.len() as u32 - 1) << 16) | (asq.data.len() as u32 - 1));
            self.regs.asq.write(asq.data.physical() as u64);
            self.regs.acq.write(acq.data.physical() as u64);

            // Set IOCQES, IOSQES, AMS, MPS, and CSS
            let mut cc = self.regs.cc.read();
            cc &= 0xFF00000F;
            cc |= (4 << 20) | (6 << 16);
            self.regs.cc.write(cc);
        }

        println!("  - Enable");
        self.regs.cc.writef(1, true);

        println!("  - Waiting for ready");
        while ! self.regs.csts.readf(1) {
            thread::yield_now();
        }

        {
            //TODO: Use buffer
            let data: Dma<[u8; 4096]> = Dma::zeroed().unwrap();

            println!("  - Attempting to identify controller");
            {
                let qid = 0;
                let queue = &mut self.submission_queues[qid];
                let cid = queue.i as u16;
                let entry = NvmeCmd::identify_controller(
                    cid, data.physical()
                );
                let tail = queue.submit(entry);
                self.submission_queue_tail(qid as u16, tail as u16);
            }

            println!("  - Waiting to identify controller");
            {
                let qid = 0;
                let queue = &mut self.completion_queues[qid];
                let (head, entry) = queue.complete_spin();
                self.completion_queue_head(qid as u16, head as u16);
            }

            println!("  - Dumping identify controller");

            let mut serial = String::new();
            for &b in &data[4..24] {
                if b == 0 {
                    break;
                }
                serial.push(b as char);
            }
            println!("    - Serial: {}", serial);

            let mut model = String::new();
            for &b in &data[24..64] {
                if b == 0 {
                    break;
                }
                model.push(b as char);
            }
            println!("    - Model: {}", model);

            let mut firmware = String::new();
            for &b in &data[64..72] {
                if b == 0 {
                    break;
                }
                firmware.push(b as char);
            }
            println!("    - Firmware: {}", firmware);
        }

        let mut nsids = Vec::new();
        {
            //TODO: Use buffer
            let data: Dma<[u32; 1024]> = Dma::zeroed().unwrap();

            println!("  - Attempting to retrieve namespace ID list");
            {
                let qid = 0;
                let queue = &mut self.submission_queues[qid];
                let cid = queue.i as u16;
                let entry = NvmeCmd::identify_namespace_list(
                    cid, data.physical(), 0
                );
                let tail = queue.submit(entry);
                self.submission_queue_tail(qid as u16, tail as u16);
            }

            println!("  - Waiting to retrieve namespace ID list");
            {
                let qid = 0;
                let queue = &mut self.completion_queues[qid];
                let (head, entry) = queue.complete_spin();
                self.completion_queue_head(qid as u16, head as u16);
            }

            println!("  - Dumping namespace ID list");
            for &nsid in data.iter() {
                if nsid != 0 {
                    println!("    - {}", nsid);
                    nsids.push(nsid);
                }
            }
        }

        let mut namespaces = BTreeMap::new();
        for &nsid in nsids.iter() {
            //TODO: Use buffer
            let data: Dma<[u8; 4096]> = Dma::zeroed().unwrap();

            println!("  - Attempting to identify namespace {}", nsid);
            {
                let qid = 0;
                let queue = &mut self.submission_queues[qid];
                let cid = queue.i as u16;
                let entry = NvmeCmd::identify_namespace(
                    cid, data.physical(), nsid
                );
                let tail = queue.submit(entry);
                self.submission_queue_tail(qid as u16, tail as u16);
            }

            println!("  - Waiting to identify namespace {}", nsid);
            {
                let qid = 0;
                let queue = &mut self.completion_queues[qid];
                let (head, entry) = queue.complete_spin();
                self.completion_queue_head(qid as u16, head as u16);
            }

            println!("  - Dumping identify namespace");

            let size = *(data.as_ptr().offset(0) as *const u64);
            println!("    - Size: {}", size);

            let capacity = *(data.as_ptr().offset(8) as *const u64);
            println!("    - Capacity: {}", capacity);

            //TODO: Read block size

            namespaces.insert(nsid, NvmeNamespace {
                id: nsid,
                blocks: size,
                block_size: 512, // TODO
            });
        }

        for io_qid in 1..self.completion_queues.len() {
            let (ptr, len) = {
                let queue = &self.completion_queues[io_qid];
                (queue.data.physical(), queue.data.len())
            };

            println!("  - Attempting to create I/O completion queue {}", io_qid);
            {
                let qid = 0;
                let queue = &mut self.submission_queues[qid];
                let cid = queue.i as u16;
                let entry = NvmeCmd::create_io_completion_queue(
                    cid, io_qid as u16, ptr, (len - 1) as u16
                );
                let tail = queue.submit(entry);
                self.submission_queue_tail(qid as u16, tail as u16);
            }

            println!("  - Waiting to create I/O completion queue {}", io_qid);
            {
                let qid = 0;
                let queue = &mut self.completion_queues[qid];
                let (head, entry) = queue.complete_spin();
                self.completion_queue_head(qid as u16, head as u16);
            }
        }

        for io_qid in 1..self.submission_queues.len() {
            let (ptr, len) = {
                let queue = &self.submission_queues[io_qid];
                (queue.data.physical(), queue.data.len())
            };

            println!("  - Attempting to create I/O submission queue {}", io_qid);
            {
                let qid = 0;
                let queue = &mut self.submission_queues[qid];
                let cid = queue.i as u16;
                //TODO: Get completion queue ID through smarter mechanism
                let entry = NvmeCmd::create_io_submission_queue(
                    cid, io_qid as u16, ptr, (len - 1) as u16, io_qid as u16
                );
                let tail = queue.submit(entry);
                self.submission_queue_tail(qid as u16, tail as u16);
            }

            println!("  - Waiting to create I/O submission queue {}", io_qid);
            {
                let qid = 0;
                let queue = &mut self.completion_queues[qid];
                let (head, entry) = queue.complete_spin();
                self.completion_queue_head(qid as u16, head as u16);
            }
        }

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
