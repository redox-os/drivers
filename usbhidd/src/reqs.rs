use std::slice;

use rehid::report_desc::ReportTy;
use xhcid_interface::{
    DeviceReqData, PortReqRecipient, PortReqTy, XhciClientHandle, XhciClientHandleError,
};

const GET_REPORT_REQ: u8 = 0x1;
const SET_REPORT_REQ: u8 = 0x9;
const GET_IDLE_REQ: u8 = 0x2;
const SET_IDLE_REQ: u8 = 0xA;
const GET_PROTOCOL_REQ: u8 = 0x3;
const SET_PROTOCOL_REQ: u8 = 0xB;

fn concat(hi: u8, lo: u8) -> u16 {
    (u16::from(hi) << 8) | u16::from(lo)
}

pub fn get_report(
    handle: &XhciClientHandle,
    report_ty: ReportTy,
    report_id: u8,
    if_num: u16,
    buffer: &mut [u8],
) -> Result<(), XhciClientHandleError> {
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        GET_REPORT_REQ,
        concat(report_ty as u8, report_id),
        if_num,
        DeviceReqData::In(buffer),
    )
}
pub fn set_report(
    handle: &XhciClientHandle,
    report_ty: ReportTy,
    report_id: u8,
    if_num: u16,
    buffer: &[u8],
) -> Result<(), XhciClientHandleError> {
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        SET_REPORT_REQ,
        concat(report_id, report_ty as u8),
        if_num,
        DeviceReqData::Out(buffer),
    )
}
pub fn get_idle(
    handle: &XhciClientHandle,
    report_id: u8,
    if_num: u16,
) -> Result<u8, XhciClientHandleError> {
    let mut idle_rate = 0;
    let buffer = slice::from_mut(&mut idle_rate);
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        GET_IDLE_REQ,
        u16::from(report_id),
        if_num,
        DeviceReqData::In(buffer),
    )?;
    Ok(idle_rate)
}
pub fn set_idle(
    handle: &XhciClientHandle,
    duration: u8,
    report_id: u8,
    if_num: u16,
) -> Result<(), XhciClientHandleError> {
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        SET_IDLE_REQ,
        concat(duration, report_id),
        if_num,
        DeviceReqData::NoData,
    )
}
pub fn get_protocol(handle: &XhciClientHandle, if_num: u16) -> Result<u8, XhciClientHandleError> {
    let mut protocol = 0;
    let buffer = slice::from_mut(&mut protocol);
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        GET_PROTOCOL_REQ,
        0,
        if_num,
        DeviceReqData::In(buffer),
    )?;
    Ok(protocol)
}
pub fn set_protocol(
    handle: &XhciClientHandle,
    protocol: u8,
    if_num: u16,
) -> Result<(), XhciClientHandleError> {
    handle.device_request(
        PortReqTy::Class,
        PortReqRecipient::Interface,
        SET_PROTOCOL_REQ,
        u16::from(protocol),
        if_num,
        DeviceReqData::NoData,
    )
}
