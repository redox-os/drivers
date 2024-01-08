mod scheme;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};

use pcid_interface::PcidServerHandle;

use syscall::{Packet, SchemeBlockMut};

use virtio_core::spec::VIRTIO_NET_F_MAC;
use virtio_core::transport::{Error, Transport};

use scheme::NetworkScheme;

#[derive(Debug)]
#[repr(C)]
pub struct VirtHeader {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

static_assertions::const_assert_eq!(core::mem::size_of::<VirtHeader>(), 12);

const MAX_BUFFER_LEN: usize = 65535;

fn deamon(deamon: redox_daemon::Daemon) -> Result<(), Error> {
    let mut pcid_handle = PcidServerHandle::connect_default()?;

    // Double check that we have the right device.
    //
    // 0x1000 - virtio-net
    let pci_config = pcid_handle.fetch_config()?;

    assert_eq!(pci_config.func.devid, 0x1000);
    log::info!("virtio-net: initiating startup sequence :^)");

    let device = virtio_core::probe_device(&mut pcid_handle)?;
    let device_space = device.device_space;

    // Negotiate device features:
    let mac_addr = if device.transport.check_device_feature(VIRTIO_NET_F_MAC) {
        let mac = (0..6)
            .map(|i| unsafe { core::ptr::read_volatile(device_space.add(i)) })
            .collect::<Vec<u8>>();

        let mac_str = format!(
            "{:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}:{:>02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        log::info!("virtio-net: device MAC is {mac_str}");

        device.transport.ack_driver_feature(VIRTIO_NET_F_MAC);
        mac_str
    } else {
        unimplemented!()
    };

    device.transport.finalize_features();

    // Allocate the recieve and transmit queues:
    //
    // > Empty buffers are placed in one virtqueue for receiving
    // > packets, and outgoing packets are enqueued into another
    // > for transmission in that order.
    //
    // TODO(andypython): Should we use the same IRQ vector for both?
    let rx_queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR, &device.irq_handle)?;

    let tx_queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR, &device.irq_handle)?;

    device.transport.run_device();

    let mut name = pci_config.func.name();
    name.push_str("_virtio_net");

    // Create the network scheme.
    //
    // FIXME(andypython): It should be fine to have multiple network devices.
    let socket_fd = syscall::open(
        &format!(":network"),
        syscall::O_RDWR | syscall::O_CREAT | syscall::O_CLOEXEC,
    )
    .map_err(Error::SyscallError)?;

    let mut socket_fd = unsafe { File::from_raw_fd(socket_fd as RawFd) };
    let mut scheme = NetworkScheme::new(rx_queue, tx_queue);

    let _ = netutils::setcfg("mac", &mac_addr);

    deamon.ready().expect("virtio-netd: failed to deamonize");

    loop {
        let mut packet = Packet::default();
        socket_fd
            .read(&mut packet)
            .expect("virtio-netd: failed to read packet");

        let result = scheme.handle(&mut packet);
        // `packet.a` contains the return value.
        packet.a = result.expect("virtio-netd: failed to handle packet");

        socket_fd
            .write(&packet)
            .expect("virtio-netd: failed to write packet");
    }
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

pub fn main() {
    #[cfg(target_os = "redox")]
    virtio_core::utils::setup_logging(log::LevelFilter::Trace, "virtio-netd");
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
