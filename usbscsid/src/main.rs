use std::env;
use std::mem;
use std::fs::File;
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};

use syscall::{CloneFlags, Map, MapFlags, Event, EventFlags, Packet, SchemeMut};
use syscall::io_uring::{self, IoUringSqeFlags};
use xhcid_interface::{ConfigureEndpointsReq, DeviceReqData, XhciClientHandle};

pub mod protocol;
pub mod scsi;

mod scheme;

use scheme::ScsiScheme;
use scsi::Scsi;

fn main() {
    let mut args = env::args().skip(1);

    const USAGE: &'static str = "usbscsid <scheme> <port> <protocol>";

    let scheme = args.next().expect(USAGE);
    let port = args
        .next()
        .expect(USAGE)
        .parse::<usize>()
        .expect("port has to be a number");
    let protocol = args
        .next()
        .expect(USAGE)
        .parse::<u8>()
        .expect("protocol has to be a number 0-255");

    println!(
        "USB SCSI driver spawned with scheme `{}`, port {}, protocol {}",
        scheme, port, protocol
    );

    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    let disk_scheme_name = format!(":disk/{}-{}_scsi", scheme, port);

    let mut event_queue_ioring_instance = io_uring::ConsumerInstance::new_v1()
        .with_submission_entry_count(64)   // much smaller, only a single page
        .with_completion_entry_count(1024) // 16384 bytes for completion entries, with 16 byte entry size
        .create_instance()
        .expect("failed to create event queue io_uring instance")
        .map_all()
        .expect("failed to map event io_uring buffers")
        .attach_to_kernel()
        .expect("failed to attach event queue to kernel");

    event_queue_ioring_instance.sender().as_64_mut().unwrap().try_send(unsafe { io_uring::SqEntry64::new(IoUringSqeFlags::empty(), 0, 0xDA7A).open(b"event:", (syscall::O_CREAT | syscall::O_RDWR) as u64) }).expect("usbscsid: failed to send event queue creation to kernel");
    event_queue_ioring_instance.wait(1, io_uring::IoUringEnterFlags::empty()).expect("usbscsid: failed to wait on io_uring");

    // TODO: Proper async/await framework...
    let cqe = event_queue_ioring_instance.receiver().as_64_mut().unwrap().try_recv().unwrap();
    assert_eq!(cqe.user_data, 0xDA7A);

    let xhci_iouring = {
        let mut consumer_instance = io_uring::ConsumerInstance::new_v1()
            .with_recommended_completion_entry_count()
            .with_recommended_submission_entry_count()
            .create_instance()
            .expect("failed to create io_uring instance")
            .map_all()
            .expect("failed to map io_uring ring memory locations")
            .attach(format!("{}:", scheme))
            .expect("failed to attach io_uring to xhcid");

        let mut sender = if let &mut io_uring::ConsumerGenericSender::Bits64(ref mut sender) = consumer_instance.sender() {
            sender
        } else {
            unreachable!();
        };

        sender.spin_on_send(io_uring::SqEntry64 {
            opcode: 1,
            flags: 0,
            priority: 0,
            syscall_flags: 0,
            fd: 42,
            user_data: 1337,
            len: 8192,
            addr: 0xDEADBEEF,
            offset: 16384,
            additional1: 0,
            additional2: 0,
        });

        event_queue_ioring_instance.sender().as_64_mut().unwrap().try_send(io_uring::SqEntry64::new(io_uring::IoUringSqeFlags::empty(), 0, 0).write(0, &Event {
            id: consumer_instance.ringfd(),
            flags: EventFlags::EVENT_URING,
            data: 0,
        })).expect("usbscsid: failed to send event queue submission to kernel");


        consumer_instance
    };

    // TODO: Use eventfds.
    let handle = XhciClientHandle::new(scheme, port);

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
        })
        .expect("Failed to configure endpoints");

    let mut protocol = protocol::setup(&handle, protocol, &desc, &conf_desc, &if_desc)
        .expect("Failed to setup protocol");

    let socket_fd = syscall::open(disk_scheme_name, syscall::O_RDWR | syscall::O_CREAT)
        .expect("usbscsid: failed to create disk scheme");
    let mut socket_file = unsafe { File::from_raw_fd(socket_fd as RawFd) };

    //syscall::setrens(0, 0).expect("scsid: failed to enter null namespace");
    let mut scsi = Scsi::new(&mut *protocol).expect("usbscsid: failed to setup SCSI");
    println!("SCSI initialized");
    let mut buffer = [0u8; 512];
    scsi.read(&mut *protocol, 0, &mut buffer).expect("usbscsid: failed to read block");
    println!("DISK CONTENT: {}", base64::encode(&buffer[..]));

    let mut scsi_scheme = ScsiScheme::new(&mut scsi, &mut *protocol);

    // TODO: Use nonblocking and put all pending calls in a todo VecDeque. Use an eventfd as well.
    'scheme_loop: loop {
        let mut packet = Packet::default();
        match socket_file.read(&mut packet) {
            Ok(0) => break 'scheme_loop,
            Ok(_) => (),
            Err(err) => panic!("scsid: failed to read disk scheme: {}", err),
        }
        scsi_scheme.handle(&mut packet);
        socket_file
            .write(&packet)
            .expect("scsid: failed to write packet");
    }
}
