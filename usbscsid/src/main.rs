use std::env;

use xhcid_interface::{ConfigureEndpointsReq, DeviceReqData, XhciClientHandle};

pub mod protocol;
pub mod scsi;

use scsi::cmds::StandardInquiryData;

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

    assert_eq!(std::mem::size_of::<StandardInquiryData>(), 96);
    let mut inquiry_buffer = [0u8; 259]; // additional_len = 255
    let mut command_buffer = [0u8; 6];

    let min_inquiry_len = 5u16;

    let max_inquiry_len = {
        {
            let inquiry = plain::from_mut_bytes(&mut command_buffer).unwrap();
            *inquiry = scsi::cmds::Inquiry::new(false, 0, min_inquiry_len, 0);
        }
        protocol.send_command(&command_buffer, DeviceReqData::In(&mut inquiry_buffer[..min_inquiry_len as usize])).expect("Failed to send command");
        let standard_inquiry_data: &StandardInquiryData = dbg!(plain::from_bytes(&inquiry_buffer).unwrap());
        4 + u16::from(standard_inquiry_data.additional_len)
    };
    {
        {
            let inquiry = plain::from_mut_bytes(&mut command_buffer).unwrap();
            *inquiry = scsi::cmds::Inquiry::new(false, 0, max_inquiry_len, 0);
        }
        protocol.send_command(&command_buffer, DeviceReqData::In(&mut inquiry_buffer[..max_inquiry_len as usize])).expect("Failed to send command");
        let standard_inquiry_data: &StandardInquiryData = dbg!(plain::from_bytes(&inquiry_buffer).unwrap());
    }
}
