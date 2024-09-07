use std::sync::{Arc, Mutex};
use crossbeam_channel;
use crossbeam_channel::RecvError;
use log::{error, info, trace, warn};
use crate::xhci::Xhci;

pub enum DeviceEnumerationRequest{
    Attach(u8),
    Detach(u8)
}

pub struct DeviceEnumerator{
    hci: Arc<Xhci>,
    request_queue: crossbeam_channel::Receiver<DeviceEnumerationRequest>,
}

impl DeviceEnumerator{
    pub fn new(hci: Arc<Xhci>) -> Self{
        let request_queue = hci.device_enumerator_receiver.clone();
        DeviceEnumerator{
            hci,
            request_queue
        }
    }

    pub fn run(&mut self) {

        loop {
            trace!("Start Device Enumerator Loop");
            let request = match self.request_queue.recv(){
                Ok(req) => req,
                Err(err) => {
                    panic!("Failed to received an enumeration request! error: {}", err)
                }
            };

            match request{
                DeviceEnumerationRequest::Attach(port_num) => {
                    info!("Device Enumerator received Attach request on port {}", port_num);
                    let result = futures::executor::block_on(self.hci.attach_device(port_num - 1));
                    match result{
                        Ok(_) => {}
                        Err(err) => {
                            warn!("processing of device attach request failed! Error: {}", err);
                        }
                    }
                }
                DeviceEnumerationRequest::Detach(port_num) => {
                    info!("Device Enumerator received Detach request on port {}", port_num);
                    let result = futures::executor::block_on(self.hci.detach_device((port_num - 1) as usize));
                    match result{
                        Ok(_) => {},
                        Err(err) => {
                            warn!("processing of device detach request failed! Error: {}", err);
                        }
                    }
                }
            }
        }
    }


}