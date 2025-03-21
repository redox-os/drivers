use crate::xhci::port::PortFlags;
use crate::xhci::{PortId, Xhci};
use common::io::Io;
use crossbeam_channel;
use log::{debug, info, warn};
use std::sync::Arc;
use std::time::Duration;
use syscall::EAGAIN;

//enum HubPortState{
//    PoweredOff,
//    Disabled,
//    Disconnected,
//    Reset,
//    Enabled,
//    Error,
//    Polling,
//    Compliance,
//    Loopback
//}
//
//impl HubPortState{
//    pub fn from_port_flags(flags: PortFlags, protocol_version: (u8, u8)) -> Self{
//        let pp = flags.contains(PortFlags::PORT_PP);
//        let ccs = flags.contains(PortFlags::PORT_CCS);
//        let ped = flags.contains(PortFlags::PORT_PED);
//        let pr = flags.contains(PortFlags::PORT_PR);
//
//        match protocol_version {
//            (2, _) | (1, _) => {
//                match (pp, ccs, ped, pr) {
//                    (false, false, false, false) => { HubPortState::PoweredOff },
//                    (true, false, false, false) => { HubPortState::Disconnected },
//                    (true, true, false, true) => { HubPortState::Reset },
//                    (true, true, false, false) => { HubPortState::Disabled },
//                    (true, true, true, false) => { HubPortState::Enabled },
//                    (true, true, true, true) => unreachable!(), //PED shouldnt be set when PR is set
//                    (false, _, _, _) => unreachable!(), //None of the other bits should be set when the port is off
//                    _ => unreachable!() //This state shouldn't be valid.
//                }
//            }
//            (3, _) => {
//                //TO-DO: USB3 state machine.
//                HubPortState::PoweredOff
//            },
//            (_, _) => unreachable!() //We don't support protocols > 3 yet.
//        }
//    }
//}
//
//struct RootHubPortStateMachine{
//    hci: Arc<Xhci>,
//    port_num: u8,
//    port_index: usize,
//    protocol_major_version: u8,
//    protocol_minor_version: u8,
//    state: HubPortState
//}
//
//impl RootHubPortStateMachine{
//    fn new(port_num: u8, hci: Arc<Xhci>) -> Self{
//
//        let hci = hci.clone();
//        let port_index = (port_num - 1) as usize;
//
//        //TODO: Get actual protocol version
//        let (maj, min) = (2u8, 0u8);
//
//        //TODO: Get actual flags
//        let flags = PortFlags::all();
//
//        RootHubPortStateMachine{
//            hci,
//            port_num,
//            port_index,
//            protocol_major_version: maj,
//            protocol_minor_version: min,
//            state: HubPortState::from_port_flags(flags, (maj, min))
//        }
//    }
//
//    fn execute(&mut self, port_num: u8){
//        //TO-DO: Implement the state machine.
//    }
//}

pub struct DeviceEnumerationRequest {
    pub port_id: PortId,
}

pub struct DeviceEnumerator {
    hci: Arc<Xhci>,
    request_queue: crossbeam_channel::Receiver<DeviceEnumerationRequest>,
}

impl DeviceEnumerator {
    pub fn new(hci: Arc<Xhci>) -> Self {
        let request_queue = hci.device_enumerator_receiver.clone();
        DeviceEnumerator { hci, request_queue }
    }

    pub fn run(&mut self) {
        loop {
            info!("Start Device Enumerator Loop");
            let request = match self.request_queue.recv() {
                Ok(req) => req,
                Err(err) => {
                    panic!("Failed to received an enumeration request! error: {}", err)
                }
            };

            let port_id = request.port_id;
            let port_array_index = port_id.root_hub_port_index();

            info!("Device Enumerator request for port {}", port_id);

            let (len, flags) = {
                let ports = self.hci.ports.lock().unwrap();

                let len = ports.len();

                if port_array_index >= len {
                    warn!(
                        "Received out of bounds Device Enumeration request for port {}",
                        port_id
                    );
                    continue;
                }

                (len, ports[port_array_index].flags())
            };

            if flags.contains(PortFlags::PORT_CCS) {
                info!(
                    "Received Device Connect Port Status Change Event with port flags {:?}",
                    flags
                );
                //If the port isn't enabled (i.e. it's a USB2 port), we need to reset it if it isn't resetting already
                //A USB3 port won't generate a Connect Status Change until it's already enabled, so this check
                //will always be skipped for USB3 ports
                if !flags.contains(PortFlags::PORT_PED) {
                    let disabled_state = flags.contains(PortFlags::PORT_PP)
                        && flags.contains(PortFlags::PORT_CCS)
                        && !flags.contains(PortFlags::PORT_PED)
                        && !flags.contains(PortFlags::PORT_PR);

                    if !disabled_state {
                        panic!(
                            "Port {} isn't in the disabled state! Current flags: {:?}",
                            port_id, flags
                        );
                    } else {
                        debug!("Port {} has entered the disabled state.", port_id);
                    }

                    //THIS LOCKS THE PORTS. DO NOT LOCK PORTS BEFORE THIS POINT
                    info!("Received a device connect on port {}, but it's not enabled. Resetting the port.", port_id);
                    self.hci.reset_port(port_array_index);

                    let mut ports = self.hci.ports.lock().unwrap();
                    let port = &mut ports[port_array_index];

                    port.portsc.writef(PortFlags::PORT_PRC.bits(), true);

                    std::thread::sleep(Duration::from_millis(16)); //Some controllers need some extra time to make the transition.

                    let flags = port.flags();

                    let enabled_state = flags.contains(PortFlags::PORT_PP)
                        && flags.contains(PortFlags::PORT_CCS)
                        && flags.contains(PortFlags::PORT_PED)
                        && !flags.contains(PortFlags::PORT_PR);

                    if !enabled_state {
                        warn!(
                            "Port {} isn't in the enabled state! Current flags: {:?}",
                            port_id, flags
                        );
                    } else {
                        debug!(
                            "Port {} is in the enabled state. Proceeding with enumeration",
                            port_id
                        );
                    }
                }

                let result = futures::executor::block_on(self.hci.attach_device(port_id));
                match result {
                    Ok(_) => {
                        info!("Device on port {} was attached", port_id);
                    }
                    Err(err) => {
                        if err.errno == EAGAIN {
                            info!("Received a device connect notification for an already connected device. Ignoring...")
                        } else {
                            warn!("processing of device attach request failed! Error: {}", err);
                        }
                    }
                }
            } else {
                info!(
                    "Device Enumerator received Detach request on port {} which is in state {}",
                    port_id,
                    self.hci.get_pls(port_id)
                );
                let result = futures::executor::block_on(self.hci.detach_device(port_id));
                match result {
                    Ok(_) => {
                        info!("Device on port {} was detached", port_id);
                    }
                    Err(err) => {
                        warn!("processing of device attach request failed! Error: {}", err);
                    }
                }
            }
        }
    }
}
