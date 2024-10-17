use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::{Read, Write};

use xhcid_interface::{
    plain, usb, ConfigureEndpointsReq, DevDesc, DeviceReqData, EndpDirection, EndpointTy,
    PortReqRecipient, PortReqTy, XhciClientHandle,
};

fn main() {
    common::setup_logging(
        "usb",
        "device",
        "hub",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbhubd <scheme> <port> <interface>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("Expected integer as input of port");
    let interface_num = args
        .next()
        .expect(USAGE)
        .parse::<u8>()
        .expect("Expected integer as input of interface");

    log::info!(
        "USB HUB driver spawned with scheme `{}`, port {}, interface {}",
        scheme,
        port,
        interface_num
    );

    let handle = XhciClientHandle::new(scheme, port);
    let desc: DevDesc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");
    log::info!("{:X?}", desc);

    let (conf_desc, conf_num, if_desc) = desc
        .config_descs
        .iter()
        .enumerate()
        .find_map(|(conf_num, conf_desc)| {
            let if_desc = conf_desc.interface_descs.iter().find_map(|if_desc| {
                if if_desc.number == interface_num {
                    Some(if_desc.clone())
                } else {
                    None
                }
            })?;
            Some((conf_desc.clone(), conf_num, if_desc))
        })
        .expect("Failed to find suitable configuration");

    /*TODO
    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: conf_num as u8,
            interface_desc: Some(interface_num),
            alternate_setting: Some(if_desc.alternate_setting),
        })
        .expect("Failed to configure endpoints");
    */

    let mut hub_desc = usb::HubDescriptor::default();
    handle
        .device_request(
            PortReqTy::Class,
            PortReqRecipient::Device,
            usb::SetupReq::GetDescriptor as u8,
            0,
            //TODO: should this be an index into interface_descs?
            interface_num as u16,
            DeviceReqData::In(unsafe { plain::as_mut_bytes(&mut hub_desc) }),
        )
        .expect("Failed to retrieve hub descriptor");
    log::info!("{:X?}", hub_desc);

    for port in 1..=hub_desc.ports {
        log::info!("power on port {port}");
        handle
            .device_request(
                PortReqTy::Class,
                PortReqRecipient::Other,
                usb::SetupReq::SetFeature as u8,
                usb::HubFeature::PortPower as u16,
                port as u16,
                DeviceReqData::NoData,
            )
            .expect("Failed to set port power");
    }

    for port in 1..=hub_desc.ports {
        let mut port_sts = usb::HubPortStatus::default();
        handle
            .device_request(
                PortReqTy::Class,
                PortReqRecipient::Other,
                usb::SetupReq::GetStatus as u8,
                0,
                port as u16,
                DeviceReqData::In(unsafe { plain::as_mut_bytes(&mut port_sts) }),
            )
            .expect("Failed to retrieve port status");
        log::info!("port {} status {:X?}", port, port_sts);

        if port_sts.contains(usb::HubPortStatus::CONNECTION) {
            /*TODO
            log::info!("reset port {port}");
            handle
                .device_request(
                    PortReqTy::Class,
                    PortReqRecipient::Other,
                    usb::SetupReq::SetFeature as u8,
                    usb::HubFeature::PortReset as u16,
                    port as u16,
                    DeviceReqData::NoData,
                )
                .expect("Failed to set port enable");
            */
            //TODO: address device
        }
    }

    //TODO: read interrupt port for changes
}
