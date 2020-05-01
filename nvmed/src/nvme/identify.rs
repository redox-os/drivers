use syscall::Dma;

use super::{Nvme, NvmeCmd, NvmeNamespace};

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
impl Nvme {
    /// Returns the serial number, model, and firmware, in that order.
    pub async fn identify_controller(&self) {
        // TODO: Use same buffer
        let data: Dma<IdentifyControllerData> = Dma::zeroed().unwrap();

        // println!("  - Attempting to identify controller");
        let cid = self
            .submit_admin_command(|cid| NvmeCmd::identify_controller(cid, data.physical()))
            .await;

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
            model, serial, firmware,
        );
    }
    pub async fn identify_namespace_list(&self, base: u32) -> Vec<u32> {
        // TODO: Use buffer
        let data: Dma<[u32; 1024]> = Dma::zeroed().unwrap();

        // println!("  - Attempting to retrieve namespace ID list");
        let cmd_id = self
            .submit_admin_command(|cid| {
                NvmeCmd::identify_namespace_list(cid, data.physical(), base)
            })
            .await;

        // println!("  - Waiting to retrieve namespace ID list");
        let comp = self.admin_queue_completion(cmd_id).await;

        // println!("  - Dumping namespace ID list");
        data.iter().copied().take_while(|&nsid| nsid != 0).collect()
    }
    pub async fn identify_namespace(&self, nsid: u32) -> NvmeNamespace {
        //TODO: Use buffer
        let data: Dma<[u8; 4096]> = Dma::zeroed().unwrap();

        // println!("  - Attempting to identify namespace {}", nsid);
        let cmd_id = self
            .submit_admin_command(|cid| NvmeCmd::identify_namespace(cid, data.physical(), nsid))
            .await;

        // println!("  - Waiting to identify namespace {}", nsid);
        let comp = self.admin_queue_completion(cmd_id).await;

        // println!("  - Dumping identify namespace");

        // TODO: Use struct
        let size = unsafe { *(data.as_ptr().offset(0) as *const u64) };
        let capacity = unsafe { *(data.as_ptr().offset(8) as *const u64) };
        println!("    - ID: {} Size: {} Capacity: {}", nsid, size, capacity);

        //TODO: Read block size

        NvmeNamespace {
            id: nsid,
            blocks: size,
            block_size: 512, // TODO
        }
    }
}
