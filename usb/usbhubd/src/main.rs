use std::{env, thread, time};

use xhcid_interface::{
    plain, usb, ConfigureEndpointsReq, DevDesc, DeviceReqData, PortId, PortReqRecipient, PortReqTy,
    XhciClientHandle,
};

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbhubd <scheme> <port> <interface>";

    let scheme = args.next().expect(USAGE);
    let port_id = args
        .next()
        .expect(USAGE)
        .parse::<PortId>()
        .expect("Expected port ID");
    let interface_num = args
        .next()
        .expect(USAGE)
        .parse::<u8>()
        .expect("Expected integer as input of interface");

    log::info!(
        "USB HUB driver spawned with scheme `{}`, port {}, interface {}",
        scheme,
        port_id,
        interface_num
    );

    let name = format!("{}_{}_{}_hub", scheme, port_id, interface_num);
    common::setup_logging(
        "usb",
        "device",
        &name,
        log::LevelFilter::Warn,
        common::file_level(),
    );

    let handle = XhciClientHandle::new(scheme.clone(), port_id);
    let desc: DevDesc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");

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

    // Read hub descriptor
    let (ports, usb_3) = if desc.major_version() >= 3 {
        // USB 3.0 hubs
        let mut hub_desc = usb::HubDescriptorV3::default();
        handle
            .device_request(
                PortReqTy::Class,
                PortReqRecipient::Device,
                usb::SetupReq::GetDescriptor as u8,
                u16::from(usb::HubDescriptorV3::DESCRIPTOR_KIND) << 8,
                0,
                DeviceReqData::In(unsafe { plain::as_mut_bytes(&mut hub_desc) }),
            )
            .expect("Failed to read hub descriptor");
        (hub_desc.ports, true)
    } else {
        // USB 2.0 and earlier hubs
        let mut hub_desc = usb::HubDescriptorV2::default();
        handle
            .device_request(
                PortReqTy::Class,
                PortReqRecipient::Device,
                usb::SetupReq::GetDescriptor as u8,
                u16::from(usb::HubDescriptorV2::DESCRIPTOR_KIND) << 8,
                0,
                DeviceReqData::In(unsafe { plain::as_mut_bytes(&mut hub_desc) }),
            )
            .expect("Failed to read hub descriptor");
        (hub_desc.ports, false)
    };

    // Configure as hub device
    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: conf_desc.configuration_value,
            interface_desc: None, //TODO: stalls on USB 3 hub: Some(interface_num),
            alternate_setting: None, //TODO: stalls on USB 3 hub: Some(if_desc.alternate_setting),
            hub_ports: Some(ports),
        })
        .expect("Failed to configure endpoints after reading hub descriptor");

    if usb_3 {
        handle
            .device_request(
                PortReqTy::Class,
                PortReqRecipient::Device,
                0x0c, // SET_HUB_DEPTH
                port_id.hub_depth().into(),
                0,
                DeviceReqData::NoData,
            )
            .expect("Failed to set hub depth");
    }

    // Initialize states
    struct PortState {
        port_id: PortId,
        port_sts: usb::HubPortStatus,
        handle: XhciClientHandle,
        attached: bool,
    }

    impl PortState {
        pub fn ensure_attached(&mut self, attached: bool) {
            if attached == self.attached {
                return;
            }

            if attached {
                self.handle.attach().expect("Failed to attach");
            } else {
                self.handle.detach().expect("Failed to detach");
            }

            self.attached = attached;
        }
    }

    let mut states = Vec::new();
    for port in 1..=ports {
        let child_port_id = port_id.child(port).expect("Cannot get child port ID");
        states.push(PortState {
            port_id: child_port_id,
            port_sts: if usb_3 {
                usb::HubPortStatus::V3(usb::HubPortStatusV3::default())
            } else {
                usb::HubPortStatus::V2(usb::HubPortStatusV2::default())
            },
            handle: XhciClientHandle::new(scheme.clone(), child_port_id),
            attached: false,
        });
    }

    //TODO: use change flags?
    loop {
        for port in 1..=ports {
            let port_idx: usize = port.checked_sub(1).unwrap().into();
            let state = states.get_mut(port_idx).unwrap();

            let port_sts = if usb_3 {
                let mut port_sts = usb::HubPortStatusV3::default();
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
                usb::HubPortStatus::V3(port_sts)
            } else {
                let mut port_sts = usb::HubPortStatusV2::default();
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
                usb::HubPortStatus::V2(port_sts)
            };
            if state.port_sts != port_sts {
                state.port_sts = port_sts;
                log::info!("port {} status {:X?}", port, port_sts);
            }

            // Ensure port is powered on
            if !port_sts.is_powered() {
                log::info!("power on port {port}");
                handle
                    .device_request(
                        PortReqTy::Class,
                        PortReqRecipient::Other,
                        usb::SetupReq::SetFeature as u8,
                        usb::HubPortFeature::PortPower as u16,
                        port as u16,
                        DeviceReqData::NoData,
                    )
                    .expect("Failed to set port power");
                state.ensure_attached(false);
                continue;
            }

            // Ignore disconnected port
            if !port_sts.is_connected() {
                state.ensure_attached(false);
                continue;
            }

            // Ignore port in reset
            if port_sts.is_resetting() {
                state.ensure_attached(false);
                continue;
            }

            // Ensure port is enabled
            if !port_sts.is_enabled() {
                log::info!("reset port {port}");
                handle
                    .device_request(
                        PortReqTy::Class,
                        PortReqRecipient::Other,
                        usb::SetupReq::SetFeature as u8,
                        usb::HubPortFeature::PortReset as u16,
                        port as u16,
                        DeviceReqData::NoData,
                    )
                    .expect("Failed to set port enable");
                state.ensure_attached(false);
                continue;
            }

            state.ensure_attached(true);
        }

        //TODO: use interrupts or poll faster?
        thread::sleep(time::Duration::new(1, 0));
    }

    //TODO: read interrupt port for changes
}
