use super::{Nvme, NvmeCmd, NvmeNamespace};

use common::dma::Dma;

/// See NVME spec section 5.15.2.2.
#[derive(Clone, Copy)]
#[repr(C, packed)]
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

/// See NVME spec section 5.15.2.1.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct IdentifyNamespaceData {
    pub nsze: u64,
    pub ncap: u64,
    pub nuse: u64,

    pub nsfeat: u8,
    pub nlbaf: u8,
    pub flbas: u8,
    pub mc: u8,

    pub dpc: u8,
    pub dps: u8,
    pub nmic: u8,
    pub rescap: u8,
    // 32
    pub fpi: u8,
    pub dlfeat: u8,
    pub nawun: u16,

    pub nawupf: u16,
    pub nacwu: u16,
    // 40
    pub nabsn: u16,
    pub nabo: u16,

    pub nabspf: u16,
    pub noiob: u16,
    // 48
    pub nvmcap: u128,
    // 64
    pub npwg: u16,
    pub npwa: u16,
    pub npdg: u16,
    pub npda: u16,
    // 72
    pub nows: u16,
    pub _rsvd1: [u8; 18],
    // 92
    pub anagrpid: u32,
    pub _rsvd2: [u8; 3],
    pub nsattr: u8,

    // 100
    pub nvmsetid: u16,
    pub endgid: u16,
    pub nguid: [u8; 16],
    pub eui64: u64,

    pub lba_format_support: [LbaFormat; 16],
    pub _rsvd3: [u8; 192],
    pub vendor_specific: [u8; 3712],
}

impl IdentifyNamespaceData {
    pub fn size_in_blocks(&self) -> u64 {
        self.nsze
    }
    pub fn capacity_in_blocks(&self) -> u64 {
        self.ncap
    }
    /// Guaranteed to be within 0..=15
    pub fn formatted_lba_size_idx(&self) -> usize {
        (self.flbas & 0xF) as usize
    }
    pub fn formatted_lba_size(&self) -> &LbaFormat {
        &self.lba_format_support[self.formatted_lba_size_idx()]
    }
    pub fn has_metadata_after_data(&self) -> bool {
        (self.flbas & (1 << 4)) != 0
    }
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct LbaFormat(pub u32);

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RelativePerformance {
    Best = 0b00,
    Better,
    Good,
    Degraded,
}
impl Ord for RelativePerformance {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // higher performance is better, hence reversed
        Ord::cmp(&(*self as u8), &(*other as u8)).reverse()
    }
}
impl PartialOrd for RelativePerformance {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl LbaFormat {
    pub fn relative_performance(&self) -> RelativePerformance {
        match ((self.0 >> 24) & 0b11) {
            0b00 => RelativePerformance::Best,
            0b01 => RelativePerformance::Better,
            0b10 => RelativePerformance::Good,
            0b11 => RelativePerformance::Degraded,
            _ => unreachable!(),
        }
    }
    pub fn is_available(&self) -> bool {
        self.log_lba_data_size() != 0
    }
    pub fn log_lba_data_size(&self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }
    pub fn lba_data_size(&self) -> Option<u64> {
        if self.log_lba_data_size() < 9 {
            return None;
        }
        if self.log_lba_data_size() >= 32 {
            return None;
        }
        Some(1u64 << self.log_lba_data_size())
    }
    pub fn metadata_size(&self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }
}

impl Nvme {
    /// Returns the serial number, model, and firmware, in that order.
    pub async fn identify_controller(&self) {
        // TODO: Use same buffer
        let data: Dma<IdentifyControllerData> = unsafe { Dma::zeroed().unwrap().assume_init() };

        // println!("  - Attempting to identify controller");
        let comp = self
            .submit_and_complete_admin_command(|cid| {
                NvmeCmd::identify_controller(cid, data.physical())
            })
            .await;
        log::trace!("Completion: {:?}", comp);

        // println!("  - Dumping identify controller");

        let model_cow = String::from_utf8_lossy(&data.model_no);
        let serial_cow = String::from_utf8_lossy(&data.serial_no);
        let fw_cow = String::from_utf8_lossy(&data.firmware_rev);

        let model = model_cow.trim();
        let serial = serial_cow.trim();
        let firmware = fw_cow.trim();

        log::info!(
            "  - Model: {} Serial: {} Firmware: {}",
            model,
            serial,
            firmware,
        );
    }
    pub async fn identify_namespace_list(&self, base: u32) -> Vec<u32> {
        // TODO: Use buffer
        let data: Dma<[u32; 1024]> = unsafe { Dma::zeroed().unwrap().assume_init() };

        // println!("  - Attempting to retrieve namespace ID list");
        let comp = self
            .submit_and_complete_admin_command(|cid| {
                NvmeCmd::identify_namespace_list(cid, data.physical(), base)
            })
            .await;

        log::trace!("Completion2: {:?}", comp);

        // println!("  - Dumping namespace ID list");
        data.iter().copied().take_while(|&nsid| nsid != 0).collect()
    }
    pub async fn identify_namespace(&self, nsid: u32) -> NvmeNamespace {
        //TODO: Use buffer
        let data: Dma<IdentifyNamespaceData> = unsafe { Dma::zeroed().unwrap().assume_init() };

        log::debug!("Attempting to identify namespace {nsid}");
        let comp = self
            .submit_and_complete_admin_command(|cid| {
                NvmeCmd::identify_namespace(cid, data.physical(), nsid)
            })
            .await;

        log::debug!("Dumping identify namespace");

        let size = data.size_in_blocks();
        let capacity = data.capacity_in_blocks();
        log::info!("NSID: {} Size: {} Capacity: {}", nsid, size, capacity);

        let block_size = data
            .formatted_lba_size()
            .lba_data_size()
            .expect("nvmed: error: size outside 512-2^64 range");
        log::debug!("NVME block size: {}", block_size);

        NvmeNamespace {
            id: nsid,
            blocks: size,
            block_size,
        }
    }
}
