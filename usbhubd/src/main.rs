use std::{env, thread, time};

use xhcid_interface::{
    plain, usb, ConfigureEndpointsReq, DevDesc, DeviceReqData, PortReqRecipient, PortReqTy, XhciClientHandle,
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

    let (conf_desc, if_desc) = desc
        .config_descs
        .iter()
        .find_map(|conf_desc| {
            let if_desc = conf_desc.interface_descs.iter().find_map(|if_desc| {
                if if_desc.number == interface_num {
                    Some(if_desc.clone())
                } else {
                    None
                }
            })?;
            Some((conf_desc.clone(), if_desc))
        })
        .expect("Failed to find suitable configuration");

    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: conf_desc.configuration_value,
            interface_desc: Some(interface_num),
            alternate_setting: Some(if_desc.alternate_setting),
        })
        .expect("Failed to configure endpoints");

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

    //TODO: use change flags?
    let mut last_port_statuses = vec![usb::HubPortStatus::default(); hub_desc.ports.into()]; 
    loop {
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

            {
                let port_idx: usize = port.checked_sub(1).unwrap().into();
                let last_port_sts = last_port_statuses.get_mut(port_idx).unwrap();
                if *last_port_sts != port_sts {
                    *last_port_sts = port_sts;
                    log::info!("port {} status {:X?}", port, port_sts);
                }
            }

            // Ensure port is powered on
            if !port_sts.contains(usb::HubPortStatus::POWER) {
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
                continue;
            }

            // Ignore disconnected port
            //TODO: turn off disconnected ports?
            if !port_sts.contains(usb::HubPortStatus::CONNECTION) {
                continue;
            }

            // Ignore port in reset
            if port_sts.contains(usb::HubPortStatus::RESET) {
                continue;
            }

            // Ensure port is enabled
            if !port_sts.contains(usb::HubPortStatus::ENABLE) {
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
                continue;
            }

            //TODO: address device
        }

        //TODO: use interrupts or poll faster?
        thread::sleep(time::Duration::new(1, 0));
    }

    //TODO: read interrupt port for changes
}
