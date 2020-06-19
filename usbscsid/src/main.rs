use std::env;
use std::mem;
use std::fs::File;
use std::io::prelude::*;
use std::os::unix::io::{FromRawFd, RawFd};

use syscall::{CloneFlags, Map, MapFlags, Packet, SchemeMut};
use syscall::io_uring;
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

    // Daemonize so that xhcid can continue to do other useful work (until proper IRQs,
    // async-await, and multithreading :D)
    if unsafe { syscall::clone(CloneFlags::empty()).unwrap() } != 0 {
        return;
    }

    let disk_scheme_name = format!(":disk/{}-{}_scsi", scheme, port);

    {
        let ringfd = syscall::open("io_uring:", syscall::O_CREAT).expect("failed to open uring");

        let create_info = io_uring::IoUringCreateInfo {
            version: io_uring::IoUringVersion {
                major: 1,
                minor: 0,
                patch: 0,
                build: 0,
            },
            flags: io_uring::IoUringCreateFlags::empty().bits(),
            len: mem::size_of::<io_uring::IoUringVersion>(),
            sq_entry_count: 128,
            cq_entry_count: 128,
        };

        let buf = unsafe { std::slice::from_raw_parts(&create_info as *const _ as *const u8, mem::size_of_val(&create_info)) };
        let res = syscall::write(ringfd, buf).expect("failed to setup ioring");
        if res != buf.len() {
            println!("Wrote less (wrote {}, expected {})", res, buf.len());
        }

        // mmaps
        let sq_ring = unsafe {
            syscall::fmap(ringfd, &Map {
                offset: io_uring::SQ_HEADER_MMAP_OFFSET,
                size: 4096,
                flags: MapFlags::MAP_SHARED | MapFlags::PROT_READ | MapFlags::PROT_WRITE,
            }).expect("failed ot mmap sq_ring")
        };
        let cq_ring = unsafe {
            syscall::fmap(ringfd, &Map {
                offset: io_uring::CQ_HEADER_MMAP_OFFSET,
                size: 4096,
                flags: MapFlags::MAP_SHARED | MapFlags::PROT_READ | MapFlags::PROT_WRITE,
            }).expect("failed ot mmap sq_ring")
        };
        let se_ring = unsafe {
            syscall::fmap(ringfd, &Map {
                offset: io_uring::SQ_ENTRIES_MMAP_OFFSET,
                size: 4096,
                flags: MapFlags::MAP_SHARED | MapFlags::PROT_READ | MapFlags::PROT_WRITE,
            }).expect("failed ot mmap sq_ring")
        };
        let ce_ring = unsafe {
            syscall::fmap(ringfd, &Map {
                offset: io_uring::CQ_ENTRIES_MMAP_OFFSET,
                size: 4096,
                flags: MapFlags::MAP_SHARED | MapFlags::PROT_READ | MapFlags::PROT_WRITE,
            }).expect("failed ot mmap sq_ring")
        };

        let dirfd = syscall::open(format!("{}:", scheme), syscall::O_DIRECTORY | syscall::O_CLOEXEC | syscall::O_RDONLY).expect("failed to open directory");
        println!("RUNNING *THE* SYSCALL");
        syscall::attach_iouring(ringfd, dirfd).expect("failed to attach ioring");
        println!("FINISHED RUNNING *THE* SYSCALL");
    }

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
