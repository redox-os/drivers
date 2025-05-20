use std::collections::BTreeMap;
use std::env;

use driver_block::{Disk, DiskScheme, ExecutorTrait};
use syscall::{Error, EIO};
use xhcid_interface::{ConfigureEndpointsReq, PortId, XhciClientHandle};

pub mod protocol;
pub mod scsi;

use crate::protocol::Protocol;
use crate::scsi::Scsi;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<PortId>()
        .expect("Expected port ID");
    let protocol = args
        .next()
        .expect(USAGE)
        .parse::<u8>()
        .expect("protocol has to be a number 0-255");

    println!(
        "USB SCSI driver spawned with scheme `{}`, port {}, protocol {}",
        scheme, port, protocol
    );

    redox_daemon::Daemon::new(move |d| daemon(d, scheme, port, protocol))
        .expect("usbscsid: failed to daemonize");
}
fn daemon(daemon: redox_daemon::Daemon, scheme: String, port: PortId, protocol: u8) -> ! {
    let disk_scheme_name = format!("disk.usb-{scheme}+{port}-scsi");

    // TODO: Use eventfds.
    let handle = XhciClientHandle::new(scheme.to_owned(), port);

    // FIXME should this wait notifying readiness until the disk scheme is created?
    daemon.ready().expect("usbscsid: failed to signal rediness");

    let desc = handle
        .get_standard_descs()
        .expect("Failed to get standard descriptors");

    // TODO: Perhaps the drivers should just be given the config, interface, and alternate setting
    // from xhcid.
    let (conf_desc, configuration_value, (if_desc, interface_num, alternate_setting)) = desc
        .config_descs
        .iter()
        .find_map(|config_desc| {
            let interface_desc = config_desc.interface_descs.iter().find_map(|if_desc| {
                if if_desc.class == 8 && if_desc.sub_class == 6 && if_desc.protocol == 0x50 {
                    Some((if_desc.clone(), if_desc.number, if_desc.alternate_setting))
                } else {
                    None
                }
            })?;
            Some((
                config_desc.clone(),
                config_desc.configuration_value,
                interface_desc,
            ))
        })
        .expect("Failed to find suitable configuration");

    handle
        .configure_endpoints(&ConfigureEndpointsReq {
            config_desc: configuration_value,
            interface_desc: Some(interface_num),
            alternate_setting: Some(alternate_setting),
            hub_ports: None,
        })
        .expect("Failed to configure endpoints");

    let mut protocol = protocol::setup(&handle, protocol, &desc, &conf_desc, &if_desc)
        .expect("Failed to setup protocol");

    // TODO: Let all of the USB drivers fork or be managed externally, and xhcid won't have to keep
    // track of all the drivers.
    let mut scsi = Scsi::new(&mut *protocol).expect("usbscsid: failed to setup SCSI");
    println!("SCSI initialized");
    let mut buffer = [0u8; 512];
    scsi.read(&mut *protocol, 0, &mut buffer).unwrap();
    println!("DISK CONTENT: {}", base64::encode(&buffer[..]));

    let event_queue = event::EventQueue::new().unwrap();

    event::user_data! {
        enum Event {
            Scheme,
        }
    };

    let mut scheme = DiskScheme::new(
        None,
        disk_scheme_name,
        BTreeMap::from([(
            0,
            UsbDisk {
                scsi: &mut scsi,
                protocol: &mut *protocol,
            },
        )]),
        &driver_block::FuturesExecutor,
    );

    //libredox::call::setrens(0, 0).expect("nvmed: failed to enter null namespace");

    event_queue
        .subscribe(
            scheme.event_handle().raw(),
            Event::Scheme,
            event::EventFlags::READ,
        )
        .unwrap();

    for event in event_queue {
        match event.unwrap().user_data {
            Event::Scheme => driver_block::FuturesExecutor
                .block_on(scheme.tick())
                .unwrap(),
        }
    }

    std::process::exit(0);
}

struct UsbDisk<'a> {
    scsi: &'a mut Scsi,
    protocol: &'a mut dyn Protocol,
}

impl Disk for UsbDisk<'_> {
    fn block_size(&self) -> u32 {
        self.scsi.block_size
    }

    fn size(&self) -> u64 {
        self.scsi.get_disk_size()
    }

    async fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
        match self.scsi.read(self.protocol, block, buffer) {
            Ok(bytes_read) => Ok(bytes_read as usize),
            Err(err) => {
                eprintln!("usbscsid: READ IO ERROR: {err}");
                Err(Error::new(EIO))
            }
        }
    }

    async fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
        match self.scsi.write(self.protocol, block, buffer) {
            Ok(bytes_written) => Ok(bytes_written as usize),
            Err(err) => {
                eprintln!("usbscsid: WRITE IO ERROR: {err}");
                Err(Error::new(EIO))
            }
        }
    }
}
