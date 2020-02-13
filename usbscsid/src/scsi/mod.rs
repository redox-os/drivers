use std::mem;

pub mod cmds;
pub mod opcodes;

use thiserror::Error;
use xhcid_interface::DeviceReqData;

use crate::protocol::{Protocol, ProtocolError, SendCommandStatus};
use cmds::{SenseKey, StandardInquiryData};
use opcodes::Opcode;

pub struct Scsi {
    command_buffer: [u8; 16],
    inquiry_buffer: [u8; 259],
    data_buffer: Vec<u8>,
}

const INQUIRY_CMD_LEN: u8 = 6;
const REPORT_SUPP_OPCODES_CMD_LEN: u8 = 12;
const REQUEST_SENSE_CMD_LEN: u8 = 6;
const MIN_INQUIRY_ALLOC_LEN: u16 = 5;
const MIN_REPORT_SUPP_OPCODES_ALLOC_LEN: u32 = 4;

#[derive(Debug, Error)]
pub enum ScsiError {
    #[error("protocol error when sending command: {0}")]
    ProtocolError(#[from] ProtocolError),
}

impl Scsi {
    pub fn new(protocol: &mut dyn Protocol) -> Self {
        assert_eq!(std::mem::size_of::<StandardInquiryData>(), 96);
        let mut this = Self {
            command_buffer: [0u8; 16],
            inquiry_buffer: [0u8; 259], // additional_len = 255 max
            data_buffer: Vec::new(),
        };

        // Get the max length that the device supports, of the Standard Inquiry Data.
        let max_inquiry_len = this.get_inquiry_alloc_len(protocol);
        // Get the Standard Inquiry Data.
        this.get_standard_inquiry_data(protocol, max_inquiry_len);
        this.res_standard_inquiry_data();

        dbg!(this.get_mode_sense10(protocol).unwrap());

        this
    }
    pub fn get_inquiry_alloc_len(&mut self, protocol: &mut dyn Protocol) -> u16 {
        self.get_standard_inquiry_data(protocol, MIN_INQUIRY_ALLOC_LEN);
        let standard_inquiry_data = self.res_standard_inquiry_data();
        4 + u16::from(standard_inquiry_data.additional_len)
    }
    pub fn get_standard_inquiry_data(&mut self, protocol: &mut dyn Protocol, max_inquiry_len: u16) {
        let inquiry = self.cmd_inquiry();
        *inquiry = cmds::Inquiry::new(false, 0, max_inquiry_len, 0);

        protocol.send_command(&self.command_buffer[..INQUIRY_CMD_LEN as usize], DeviceReqData::In(&mut self.inquiry_buffer[..max_inquiry_len as usize])).expect("Failed to send INQUIRY command");
    }
    /*/// Similar to `check_supp_opcode_sized`, but simply checks whether the opcode is supported,
    /// without fetching any actual data.
    pub fn check_supp_opcode(&mut self, protocol: &mut dyn Protocol, opcode: Opcode, sa: Option<u16>) -> Result<bool, ScsiError> {
        self.check_supp_opcode_sized(protocol, opcode, sa, 2)
    }
    pub fn check_supp_opcode_sized(&mut self, protocol: &mut dyn Protocol, opcode: Opcode, sa: Option<u16>, alloc_len: u32) -> Result<bool, ScsiError> {
        let report_supp_opcodes = self.cmd_report_supp_opcodes();
        *report_supp_opcodes = if let Some(serviceaction) = sa {
            cmds::ReportSuppOpcodes::get_supp(false, opcode, serviceaction, alloc_len, 0)
        } else {
            cmds::ReportSuppOpcodes::get_supp_no_sa(false, opcode, alloc_len, 0)
        };
        self.data_buffer.resize(std::mem::size_of::<cmds::OneCommandParam>(), 0);
        protocol.send_command(&self.command_buffer[..REPORT_SUPP_OPCODES_CMD_LEN as usize], DeviceReqData::In(&mut self.data_buffer[..alloc_len as usize]))?;
        Ok(self.res_one_command().support() == cmds::OneCommandParamSupport::Supported)
    }*/
    
    /*pub fn get_supp_opcodes_alloc_len(&mut self, protocol: &mut dyn Protocol) -> u32 {
        self.get_supp_opcodes(protocol, MIN_REPORT_SUPP_OPCODES_ALLOC_LEN);
        self.res_all_commands().alloc_len()
    }*/
    /*pub fn get_supp_opcodes(&mut self, protocol: &mut dyn Protocol, alloc_len: u32) {
        let report_supp_opcodes = self.cmd_report_supp_opcodes();
        *report_supp_opcodes = cmds::ReportSuppOpcodes::get_all(false, alloc_len, 0);
        self.data_buffer.resize(alloc_len as usize, 0);
        let status = protocol.send_command(&self.command_buffer[..REPORT_SUPP_OPCODES_CMD_LEN as usize], DeviceReqData::In(&mut self.data_buffer[..alloc_len as usize])).expect("Failed to send REPORT_SUPP_OPCODES command");
        if status != SendCommandStatus::Success {
            self.get_ff_sense(protocol, cmds::RequestSense::MINIMAL_ALLOC_LEN);
            let data = self.res_ff_sense_data();
            if data.sense_key() == SenseKey::IllegalRequest && data.add_sense_code == cmds::ADD_SENSE_CODE05_INVAL_CDB_FIELD {
            }
        }
    }*/
    pub fn get_ff_sense(&mut self, protocol: &mut dyn Protocol, alloc_len: u8) {
        let request_sense = self.cmd_request_sense();
        *request_sense = cmds::RequestSense::new(false, alloc_len, 0);
        self.data_buffer.resize(alloc_len.into(), 0);
        protocol.send_command(&self.command_buffer[..REQUEST_SENSE_CMD_LEN as usize], DeviceReqData::In(&mut self.data_buffer[..alloc_len as usize])).expect("Failed to send REQUEST_SENSE command");
    }
    pub fn get_mode_sense10(&mut self, protocol: &mut dyn Protocol) -> Result<(&cmds::ModeParamHeader10, BlkDescSlice), ScsiError> {
        let initial_alloc_len = 4; // covers both mode_data_len and blk_desc_len.
        let mode_sense10 = self.cmd_mode_sense10();
        *mode_sense10 = cmds::ModeSense10::get_block_desc(initial_alloc_len, 0);
        self.data_buffer.resize(mem::size_of::<cmds::ModeParamHeader10>(), 0);
        if let SendCommandStatus::Failed { .. } = protocol.send_command(&self.command_buffer[..10], DeviceReqData::In(&mut self.data_buffer[..initial_alloc_len as usize]))? {
            self.get_ff_sense(protocol, 252);
            panic!("{:?}", self.res_ff_sense_data());
        }

        let optimal_alloc_len = self.res_mode_param_header10().block_desc_len() + self.res_mode_param_header10().mode_data_len() + mem::size_of::<cmds::ModeParamHeader10>() as u16;

        let mode_sense10 = self.cmd_mode_sense10();
        *mode_sense10 = cmds::ModeSense10::get_block_desc(optimal_alloc_len, 0);
        self.data_buffer.resize(optimal_alloc_len as usize, 0);
        protocol.send_command(&self.command_buffer[..10], DeviceReqData::In(&mut self.data_buffer[..optimal_alloc_len as usize]))?;
        Ok((self.res_mode_param_header10(), self.res_blkdesc_mode10()))
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
    /*pub fn cmd_report_supp_opcodes(&mut self) -> &mut cmds::ReportSuppOpcodes {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }*/
    pub fn cmd_request_sense(&mut self) -> &mut cmds::RequestSense {
        plain::from_mut_bytes(&mut self.command_buffer).unwrap()
    }
    pub fn res_standard_inquiry_data(&self) -> &StandardInquiryData {
        plain::from_bytes(&self.inquiry_buffer).unwrap()
    }
    /*
    pub fn res_all_commands(&self) -> &cmds::AllCommandsParam {
        plain::from_bytes(&self.data_buffer).unwrap()
    }
    pub fn res_one_command(&self) -> &cmds::OneCommandParam {
        plain::from_bytes(&self.data_buffer).unwrap()
    }*/
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
        plain::slice_from_bytes(&self.data_buffer[descs_start..descs_start + usize::from(header.block_desc_len)]).unwrap()
    }
    pub fn res_blkdesc_mode10(&self) -> BlkDescSlice {
        let header = self.res_mode_param_header10();
        let descs_start = mem::size_of::<cmds::ModeParamHeader10>();
        println!("MODE_SENSE PAGES: {}", base64::encode(&self.data_buffer[descs_start + header.block_desc_len() as usize..]));
        if header.longlba() {
            BlkDescSlice::Long(plain::slice_from_bytes(&self.data_buffer[descs_start..descs_start + usize::from(header.block_desc_len)]).unwrap())
        } else {
            //BlkDescSlice::Short(plain::slice_from_bytes(&self.data_buffer[descs_start..descs_start + usize::from(header.block_desc_len)]).unwrap())
            BlkDescSlice::General(plain::slice_from_bytes(&self.data_buffer[descs_start..descs_start + usize::from(header.block_desc_len)]).unwrap())
        }
    }
    pub fn get_disk_size(&mut self) -> u64 {
        todo!()
    }
}
#[derive(Debug)]
pub enum BlkDescSlice<'a> {
    //Short(&'a [cmds::ShortLbaModeParamBlkDesc]),
    General(&'a [cmds::GeneralModeParamBlkDesc]),
    Long(&'a [cmds::LongLbaModeParamBlkDesc]),
}
