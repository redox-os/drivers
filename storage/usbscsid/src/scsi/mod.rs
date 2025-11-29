use std::convert::TryFrom;
use std::mem;

pub mod cmds;
pub mod opcodes;

use thiserror::Error;
use xhcid_interface::DeviceReqData;

use crate::protocol::{Protocol, ProtocolError, SendCommandStatus, SendCommandStatusKind};
use cmds::StandardInquiryData;

pub struct Scsi {
    command_buffer: [u8; 16],
    inquiry_buffer: [u8; 259],
    data_buffer: Vec<u8>,
    pub block_size: u32,
    pub block_count: u64,
}

const INQUIRY_CMD_LEN: u8 = 6;
const REPORT_SUPP_OPCODES_CMD_LEN: u8 = 12;
const REQUEST_SENSE_CMD_LEN: u8 = 6;
const MIN_INQUIRY_ALLOC_LEN: u16 = 5;
const MIN_REPORT_SUPP_OPCODES_ALLOC_LEN: u32 = 4;

type Result<T, E = ScsiError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ScsiError {
    // TODO: Add some kind of context here, since it's very useful indeed to be able to see which
    // command returned the protocol error.
    #[error("protocol error when sending command: {0}")]
    ProtocolError(#[from] ProtocolError),

    #[error("overflow")]
    Overflow(&'static str),
}

impl Scsi {
    pub fn new(protocol: &mut dyn Protocol) -> Result<Self> {
        assert_eq!(std::mem::size_of::<StandardInquiryData>(), 96);

        let mut this = Self {
            command_buffer: [0u8; 16],
            // separate buffer since the inquiry data is most likely going to be used in the
            // future.
            inquiry_buffer: [0u8; 259], // additional_len = 255 max
            data_buffer: Vec::new(),
            block_size: 0,
            block_count: 0,
        };

        // Get the max length that the device supports, of the Standard Inquiry Data.
        let max_inquiry_len = this.get_inquiry_alloc_len(protocol)?;
        // Get the Standard Inquiry Data.
        this.get_standard_inquiry_data(protocol, max_inquiry_len)?;

        let version = this.res_standard_inquiry_data().version();
        println!("Inquiry version: {}", version);

        let (block_size, block_count) = {
            let (_, blkdescs, mode_page_iter) = this.get_mode_sense10(protocol)?;

            for page in mode_page_iter {
                println!("PAGE: {:?}", page);
            }

            // TODO: Can there be multiple disks at all?
            if let Some(only_blkdesc) = blkdescs.get(0) {
                println!("Found block desc: {:?}", only_blkdesc);
                (only_blkdesc.block_size(), only_blkdesc.block_count())
            } else {
                println!("read_capacity10");
                let r = this.read_capacity(protocol)?;
                println!("read_capacity10 result: {:?}", r);
                (r.logical_block_len(), r.block_count().into())
            }
        };

        this.block_size = block_size;
        this.block_count = block_count;

        Ok(this)
    }
    pub fn get_inquiry_alloc_len(&mut self, protocol: &mut dyn Protocol) -> Result<u16> {
        self.get_standard_inquiry_data(protocol, MIN_INQUIRY_ALLOC_LEN)?;
        let standard_inquiry_data = self.res_standard_inquiry_data();
        Ok(4 + u16::from(standard_inquiry_data.additional_len))
    }
    pub fn get_standard_inquiry_data(
        &mut self,
        protocol: &mut dyn Protocol,
        max_inquiry_len: u16,
    ) -> Result<()> {
        let inquiry = self.cmd_inquiry();
        *inquiry = cmds::Inquiry::new(false, 0, max_inquiry_len, 0);

        protocol.send_command(
            &self.command_buffer[..INQUIRY_CMD_LEN as usize],
            DeviceReqData::In(&mut self.inquiry_buffer[..max_inquiry_len as usize]),
        )?;
        Ok(())
    }
    pub fn get_ff_sense(&mut self, protocol: &mut dyn Protocol, alloc_len: u8) -> Result<()> {
        let request_sense = self.cmd_request_sense();
        *request_sense = cmds::RequestSense::new(false, alloc_len, 0);
        self.data_buffer.resize(alloc_len.into(), 0);
        protocol.send_command(
            &self.command_buffer[..REQUEST_SENSE_CMD_LEN as usize],
            DeviceReqData::In(&mut self.data_buffer[..alloc_len as usize]),
        )?;
        Ok(())
    }
    pub fn read_capacity(
        &mut self,
        protocol: &mut dyn Protocol,
    ) -> Result<&cmds::ReadCapacity10ParamData> {
        // The spec explicitly states that the allocation length is 8 bytes.
        let read_capacity10 = self.cmd_read_capacity10();
        *read_capacity10 = cmds::ReadCapacity10::new(0);
        self.data_buffer.resize(10usize, 0u8);
        protocol.send_command(
            &self.command_buffer[..10],
            DeviceReqData::In(&mut self.data_buffer[..8]),
        )?;
        Ok(self.res_read_capacity10())
    }
    pub fn get_mode_sense10(
        &mut self,
        protocol: &mut dyn Protocol,
    ) -> Result<(
        &cmds::ModeParamHeader10,
        BlkDescSlice<'_>,
        impl Iterator<Item = cmds::AnyModePage<'_>>,
    )> {
        let initial_alloc_len = mem::size_of::<cmds::ModeParamHeader10>() as u16; // covers both mode_data_len and blk_desc_len.
        let mode_sense10 = self.cmd_mode_sense10();
        *mode_sense10 = cmds::ModeSense10::get_block_desc(initial_alloc_len, 0);
        self.data_buffer
            .resize(mem::size_of::<cmds::ModeParamHeader10>(), 0);
        if let SendCommandStatus {
            kind: SendCommandStatusKind::Failed,
            ..
        } = protocol.send_command(
            &self.command_buffer[..10],
            DeviceReqData::In(&mut self.data_buffer[..initial_alloc_len as usize]),
        )? {
            self.get_ff_sense(protocol, 252)?;
            panic!("{:?}", self.res_ff_sense_data());
        }

        let optimal_alloc_len = self.res_mode_param_header10().mode_data_len() + 2; // the length of the mode data field itself

        let mode_sense10 = self.cmd_mode_sense10();
        *mode_sense10 = cmds::ModeSense10::get_block_desc(optimal_alloc_len, 0);
        self.data_buffer.resize(optimal_alloc_len as usize, 0);
        protocol.send_command(
            &self.command_buffer[..10],
            DeviceReqData::In(&mut self.data_buffer[..optimal_alloc_len as usize]),
        )?;
        Ok((
            self.res_mode_param_header10(),
            self.res_blkdesc_mode10(),
            self.res_mode_pages10(),
        ))
    }

    pub fn cmd_inquiry(&mut self) -> &mut cmds::Inquiry {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_mode_sense6(&mut self) -> &mut cmds::ModeSense6 {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_mode_sense10(&mut self) -> &mut cmds::ModeSense10 {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_request_sense(&mut self) -> &mut cmds::RequestSense {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_read_capacity10(&mut self) -> &mut cmds::ReadCapacity10 {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_read16(&mut self) -> &mut cmds::Read16 {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn cmd_write16(&mut self) -> &mut cmds::Write16 {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn res_standard_inquiry_data(&self) -> &StandardInquiryData {
        plain::from_bytes(&self.inquiry_buffer).unwrap()
    }
    pub fn res_ff_sense_data(&self) -> &cmds::FixedFormatSenseData {
        plain::from_bytes(&self.data_buffer).unwrap()
    }
    pub fn res_mode_param_header6(&self) -> &cmds::ModeParamHeader6 {
        plain::from_bytes(&self.data_buffer).unwrap()
    }
    pub fn res_mode_param_header10(&self) -> &cmds::ModeParamHeader10 {
        plain::from_bytes(&self.data_buffer).unwrap()
    }
    pub fn res_blkdesc_mode6(&self) -> &[cmds::ShortLbaModeParamBlkDesc] {
        let header = self.res_mode_param_header6();
        let descs_start = mem::size_of::<cmds::ModeParamHeader6>();
        plain::slice_from_bytes(
            &self.data_buffer[descs_start..descs_start + usize::from(header.block_desc_len)],
        )
        .unwrap()
    }
    pub fn res_blkdesc_mode10(&self) -> BlkDescSlice<'_> {
        let header = self.res_mode_param_header10();
        let descs_start = mem::size_of::<cmds::ModeParamHeader10>();
        if header.longlba() {
            BlkDescSlice::Long(
                plain::slice_from_bytes(
                    &self.data_buffer
                        [descs_start..descs_start + usize::from(header.block_desc_len())],
                )
                .unwrap(),
            )
        } else if self.res_standard_inquiry_data().periph_dev_ty()
            != cmds::PeriphDeviceType::DirectAccess as u8
            && self.res_standard_inquiry_data().version() == cmds::InquiryVersion::Spc3 as u8
        {
            BlkDescSlice::General(
                plain::slice_from_bytes(
                    &self.data_buffer
                        [descs_start..descs_start + usize::from(header.block_desc_len())],
                )
                .unwrap(),
            )
        } else {
            BlkDescSlice::Short(
                plain::slice_from_bytes(
                    &self.data_buffer
                        [descs_start..descs_start + usize::from(header.block_desc_len())],
                )
                .unwrap(),
            )
        }
    }

    pub fn res_mode_pages10(&self) -> impl Iterator<Item = cmds::AnyModePage<'_>> {
        let header = self.res_mode_param_header10();
        let descs_start = mem::size_of::<cmds::ModeParamHeader10>();
        let buffer = &self.data_buffer[descs_start + header.block_desc_len() as usize..];
        cmds::mode_page_iter(buffer)
    }
    pub fn res_read_capacity10(&self) -> &cmds::ReadCapacity10ParamData {
        plain::from_bytes(&self.data_buffer).unwrap()
    }
    pub fn get_disk_size(&self) -> u64 {
        self.block_count * u64::from(self.block_size)
    }
    pub fn read(
        &mut self,
        protocol: &mut dyn Protocol,
        lba: u64,
        buffer: &mut [u8],
    ) -> Result<u32> {
        let blocks_to_read = buffer.len() as u64 / u64::from(self.block_size);
        let bytes_to_read = blocks_to_read as usize * self.block_size as usize;
        let transfer_len = u32::try_from(blocks_to_read).or(Err(ScsiError::Overflow(
            "number of blocks to read couldn't fit inside a u32",
        )))?;
        {
            let read = self.cmd_read16();
            *read = cmds::Read16::new(lba, transfer_len, 0);
        }
        // TODO: Use the to-be-written TransferReadStream instead of relying on everything being
        // able to fit within a single buffer.
        self.data_buffer.resize(bytes_to_read, 0u8);
        let status = protocol.send_command(
            &self.command_buffer[..16],
            DeviceReqData::In(&mut self.data_buffer[..bytes_to_read]),
        )?;
        buffer[..bytes_to_read].copy_from_slice(&self.data_buffer[..bytes_to_read]);
        Ok(status.bytes_transferred(bytes_to_read as u32))
    }
    pub fn write(&mut self, protocol: &mut dyn Protocol, lba: u64, buffer: &[u8]) -> Result<u32> {
        let blocks_to_write = buffer.len() as u64 / u64::from(self.block_size);
        let bytes_to_write = blocks_to_write as usize * self.block_size as usize;
        let transfer_len = u32::try_from(blocks_to_write).or(Err(ScsiError::Overflow(
            "number of blocks to write couldn't fit inside a u32",
        )))?;
        {
            let read = self.cmd_write16();
            *read = cmds::Write16::new(lba, transfer_len, 0);
        }
        // TODO: Use the to-be-written TransferReadStream instead of relying on everything being
        // able to fit within a single buffer.
        self.data_buffer.resize(bytes_to_write, 0u8);
        self.data_buffer[..bytes_to_write].copy_from_slice(&buffer[..bytes_to_write]);
        let status = protocol.send_command(
            &self.command_buffer[..16],
            DeviceReqData::Out(&buffer[..bytes_to_write]),
        )?;
        Ok(status.bytes_transferred(bytes_to_write as u32))
    }
}
#[derive(Debug)]
pub enum BlkDescSlice<'a> {
    Short(&'a [cmds::ShortLbaModeParamBlkDesc]),
    General(&'a [cmds::GeneralModeParamBlkDesc]),
    Long(&'a [cmds::LongLbaModeParamBlkDesc]),
}

#[derive(Debug)]
pub enum BlkDesc<'a> {
    Short(&'a cmds::ShortLbaModeParamBlkDesc),
    General(&'a cmds::GeneralModeParamBlkDesc),
    Long(&'a cmds::LongLbaModeParamBlkDesc),
}
impl<'a> BlkDesc<'a> {
    fn block_size(&self) -> u32 {
        match self {
            Self::Short(s) => s.logical_block_len(),
            Self::General(g) => g.logical_block_len(),
            Self::Long(l) => l.logical_block_len(),
        }
    }
    fn block_count(&self) -> u64 {
        match self {
            Self::Short(s) => s.block_count().into(),
            Self::General(g) => g.block_count().into(),
            Self::Long(l) => l.block_count(),
        }
    }
}

impl<'a> BlkDescSlice<'a> {
    fn get(&self, idx: usize) -> Option<BlkDesc<'a>> {
        match self {
            Self::Short(s) => s.get(idx).map(BlkDesc::Short),
            Self::Long(l) => l.get(idx).map(BlkDesc::Long),
            Self::General(g) => g.get(idx).map(BlkDesc::General),
        }
    }
}
