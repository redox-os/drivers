use std::{env, thread, time};

use xhcid_interface::{
    plain, usb, ConfigureEndpointsReq, DevDesc, DeviceReqData, PortId, PortReqRecipient, PortReqTy,
    XhciClientHandle,
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
    
    //TODO: is it required to configure before reading hub descriptor?

    // Read hub descriptor
    let mut hub_desc = usb::HubDescriptor::default();
    handle
        .device_request(
            PortReqTy::Class,
            PortReqRecipient::Device,
            usb::SetupReq::GetDescriptor as u8,
            0,
            0,
            DeviceReqData::In(unsafe { plain::as_mut_bytes(&mut hub_desc) }),
        )
        .expect("Failed to read hub descriptor");

    // Configure as hub device
    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: conf_desc.configuration_value,
            interface_desc: Some(interface_num),
            alternate_setting: Some(if_desc.alternate_setting),
            hub_ports: Some(hub_desc.ports),
        })
        .expect("Failed to configure endpoints after reading hub descriptor");

    /*TODO: only set hub depth on USB 3+ hubs
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
    */

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
    for port in 1..=hub_desc.ports {
        let child_port_id = port_id.child(port).expect("Cannot get child port ID");
        states.push(PortState {
            port_id: child_port_id,
            port_sts: usb::HubPortStatus::default(),
            handle: XhciClientHandle::new(scheme.clone(), child_port_id),
            attached: false,
        });
    }

    //TODO: use change flags?
    loop {
        for port in 1..=hub_desc.ports {
            let port_idx: usize = port.checked_sub(1).unwrap().into();
            let mut state = states.get_mut(port_idx).unwrap();

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
            if state.port_sts != port_sts {
                state.port_sts = port_sts;
                log::info!("port {} status {:X?}", port, port_sts);
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
                state.ensure_attached(false);
                continue;
            }

            // Ignore disconnected port
            if !port_sts.contains(usb::HubPortStatus::CONNECTION) {
                state.ensure_attached(false);
                continue;
            }

            // Ignore port in reset
            if port_sts.contains(usb::HubPortStatus::RESET) {
                state.ensure_attached(false);
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
