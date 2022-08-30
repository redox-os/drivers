use super::NvmeCmd;

impl NvmeCmd {
    pub fn create_io_completion_queue(
        cid: u16,
        qid: u16,
        ptr: usize,
        size: u16,
        iv: Option<u16>,
    ) -> Self {
        const DW11_PHYSICALLY_CONTIGUOUS_BIT: u32 = 0x0000_0001;
        const DW11_ENABLE_INTERRUPTS_BIT: u32 = 0x0000_0002;
        const DW11_INTERRUPT_VECTOR_SHIFT: u8 = 16;

        Self {
            opcode: 5,
            flags: 0,
            cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: ((size as u32) << 16) | (qid as u32),

            cdw11: DW11_PHYSICALLY_CONTIGUOUS_BIT
                | if let Some(iv) = iv {
                    // enable interrupts if a vector is present
                    DW11_ENABLE_INTERRUPTS_BIT | (u32::from(iv) << DW11_INTERRUPT_VECTOR_SHIFT)
                } else {
                    0
                },

            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn create_io_submission_queue(
        cid: u16,
        qid: u16,
        ptr: usize,
        size: u16,
        cqid: u16,
    ) -> Self {
        Self {
            opcode: 1,
            flags: 0,
            cid,
            nsid: 0,
            _rsvd: 0,
            mptr: 0,
            dptr: [ptr as u64, 0],
            cdw10: ((size as u32) << 16) | (qid as u32),
            cdw11: ((cqid as u32) << 16) | 1, /* Physically Contiguous */
            //TODO: QPRIO
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
            ..Default::default()
        }
    }

    pub fn io_read(cid: u16, nsid: u32, lba: u64, blocks_1: u16, ptr0: u64, ptr1: u64) -> Self {
        Self {
            opcode: 2,
            flags: 0,
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
            flags: 0,
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
