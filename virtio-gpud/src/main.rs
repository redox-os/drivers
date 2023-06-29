use pcid_interface::PcidServerHandle;
use virtio_core::transport::Error;

fn deamon(deamon: redox_daemon::Daemon) -> Result<(), Error> {
    let mut pcid_handle = PcidServerHandle::connect_default()?;

    // Double check that we have the right device.
    //
    // 0x1050 - virtio-gpu
    let pci_config = pcid_handle.fetch_config()?;

    assert_eq!(pci_config.func.devid, 0x1050);
    log::info!("virtio-gpu: initiating startup sequence :^)");

    let device = virtio_core::probe_device(&mut pcid_handle)?;
    loop {}
}

fn daemon_runner(redox_daemon: redox_daemon::Daemon) -> ! {
    deamon(redox_daemon).unwrap();
    unreachable!();
}

pub fn main() {
    #[cfg(target_os = "redox")]
    virtio_core::utils::setup_logging(log::LevelFilter::Trace, "virtio-gpud");
    redox_daemon::Daemon::new(daemon_runner).expect("virtio-core: failed to daemonize");
}
