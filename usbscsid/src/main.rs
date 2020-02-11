use std::env;

use xhcid_interface::{ConfigureEndpointsReq, DeviceReqData, XhciClientHandle};

pub mod protocol;
pub mod scsi;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args.next().expect(USAGE).parse::<usize>().expect("port has to be a number");
    let protocol = args.next().expect(USAGE).parse::<u8>().expect("protocol has to be a number 0-255");

    println!("USB SCSI driver spawned with scheme `{}`, port {}, protocol {}", scheme, port, protocol);

    let handle = XhciClientHandle::new(scheme, port);

    let desc = handle.get_standard_descs().expect("Failed to get standard descriptors");

    // TODO: Perhaps the drivers should just be given the config, interface, and alternate setting
    // from xhcid.
    let (conf_desc, configuration_value, (if_desc, interface_num, alternate_setting)) = desc.config_descs.iter().find_map(|config_desc| {
        let interface_desc = config_desc.interface_descs.iter().find_map(|if_desc| if if_desc.class == 8 && if_desc.sub_class == 6 && if_desc.protocol == 0x50 {
            Some((if_desc.clone(), if_desc.number, if_desc.alternate_setting))
        } else {
            None
        })?;
        Some((config_desc.clone(), config_desc.configuration_value, interface_desc))
    }).expect("Failed to find suitable configuration");

    handle.configure_endpoints(&ConfigureEndpointsReq {
        config_desc: configuration_value,
        interface_desc: Some(interface_num),
        alternate_setting: Some(alternate_setting),
    }).expect("Failed to configure endpoints");

    let mut protocol = protocol::setup(&handle, protocol, &desc, &conf_desc, &if_desc).expect("Failed to setup protocol");

    /*let get_info = {
        // Max number of bytes that can be recieved from a "REPORT IDENTIFYING INFORMATION"
        // command.
        let alloc_len = 256; 
        let info_ty = scsi::cmds::ReportIdInfoInfoTy::IdentInfoSupp;
        let control = 0; // TODO: NACA?
        scsi::cmds::ReportIdentInfo::new(alloc_len, info_ty, control)
    };*/
    let mut buffer = [0u8; 5];
    let mut command_buffer = [0u8; 6];
    {
        let mut inquiry = plain::from_mut_bytes(&mut command_buffer).unwrap();
        *inquiry = scsi::cmds::Inquiry::new(false, 0, 5, 0);
    }
    protocol.send_command(&command_buffer, DeviceReqData::In(&mut buffer)).expect("Failed to send command");
}
