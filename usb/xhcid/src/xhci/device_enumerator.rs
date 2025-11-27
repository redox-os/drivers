use crate::xhci::port::PortFlags;
use crate::xhci::{PortId, Xhci};
use common::io::Io;
use crossbeam_channel;
use log::{debug, info, warn};
use std::sync::Arc;
use std::time::Duration;
use syscall::EAGAIN;

pub struct DeviceEnumerationRequest {
    pub port_id: PortId,
}

pub struct DeviceEnumerator<const N: usize> {
    hci: Arc<Xhci<N>>,
    request_queue: crossbeam_channel::Receiver<DeviceEnumerationRequest>,
}

impl<const N: usize> DeviceEnumerator<N> {
    pub fn new(hci: Arc<Xhci<N>>) -> Self {
        let request_queue = hci.device_enumerator_receiver.clone();
        DeviceEnumerator { hci, request_queue }
    }

    pub fn run(&mut self) {
        loop {
            debug!("Start Device Enumerator Loop");
            let request = match self.request_queue.recv() {
                Ok(req) => req,
                Err(err) => {
                    panic!("Failed to received an enumeration request! error: {}", err)
                }
            };

            let port_id = request.port_id;
            let port_array_index = port_id.root_hub_port_index();

            debug!("Device Enumerator request for port {}", port_id);

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

            if flags.contains(PortFlags::CCS) {
                debug!(
                    "Received Device Connect Port Status Change Event with port flags {:?}",
                    flags
                );
                //If the port isn't enabled (i.e. it's a USB2 port), we need to reset it if it isn't resetting already
                //A USB3 port won't generate a Connect Status Change until it's already enabled, so this check
                //will always be skipped for USB3 ports
                if !flags.contains(PortFlags::PED) {
                    let disabled_state = flags.contains(PortFlags::PP)
                        && flags.contains(PortFlags::CCS)
                        && !flags.contains(PortFlags::PED)
                        && !flags.contains(PortFlags::PR);

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
                    let _ = self.hci.reset_port(port_id);

                    let mut ports = self.hci.ports.lock().unwrap();
                    let port = &mut ports[port_array_index];

                    port.clear_prc();

                    std::thread::sleep(Duration::from_millis(16)); //Some controllers need some extra time to make the transition.

                    let flags = port.flags();

                    let enabled_state = flags.contains(PortFlags::PP)
                        && flags.contains(PortFlags::CCS)
                        && flags.contains(PortFlags::PED)
                        && !flags.contains(PortFlags::PR);

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
                            debug!("Received a device connect notification for an already connected device. Ignoring...")
                        } else {
                            warn!("processing of device attach request failed! Error: {}", err);
                        }
                    }
                }
            } else {
                debug!(
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
