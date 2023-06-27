use pcid_interface::PcidServerHandle;

use virtio_core::spec::VIRTIO_NET_F_MAC;
use virtio_core::transport::Error;

fn deamon(_deamon: redox_daemon::Daemon) -> Result<(), Error> {
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
    if device.transport.check_device_feature(VIRTIO_NET_F_MAC) {
        let mac = (0..6)
            .map(|i| unsafe { core::ptr::read_volatile(device_space.add(i)) })
            .collect::<Vec<u8>>();

        log::info!(
            "virtio-net: device MAC is {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );

        device.transport.ack_driver_feature(VIRTIO_NET_F_MAC);
    }

    // Allocate the recieve and transmit queues:
    //
    // TODO(andypython): Should we use the same IRQ vector for both?
    let rx_queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR)?;

    let tx_queue = device
        .transport
        .setup_queue(virtio_core::MSIX_PRIMARY_VECTOR)?;

    device.transport.finalize_features();
    loop {}
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
