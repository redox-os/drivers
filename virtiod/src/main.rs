use pcid_interface::{PciFeature, PcidServerHandle};

pub fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "redox")]
    setup_logging();

    redox_daemon::Daemon::new(daemon).expect("virtio-core: failed to daemonize");
}

#[cfg(target_os = "redox")]
fn setup_logging() {
    use redox_log::{OutputBuilder, RedoxLogger};

    let mut logger = RedoxLogger::new().with_output(
        OutputBuilder::stderr()
            // limit global output to important info
            .with_filter(log::LevelFilter::Info)
            .with_ansi_escape_codes()
            .flush_on_newline(true)
            .build(),
    );

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "virtiod.log") {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(log::LevelFilter::Info)
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(err) => eprintln!("virtiod: failed to create log: {}", err),
    }

    match OutputBuilder::in_redox_logging_scheme("disk", "pcie", "virtiod.ansi.log") {
        Ok(builder) => {
            logger = logger.with_output(
                builder
                    .with_filter(log::LevelFilter::Info)
                    .with_ansi_escape_codes()
                    .flush_on_newline(true)
                    .build(),
            )
        }
        Err(err) => eprintln!("virtiod: failed to create ansi log: {}", err),
    }

    logger.enable().unwrap();
    log::info!("virtiod: enabled logger");
}

fn daemon(_deamon: redox_daemon::Daemon) -> ! {
    let mut pcid_handle = PcidServerHandle::connect_default().unwrap();
    let pci_config = pcid_handle.fetch_config().unwrap();

    // 0x1001 - virtio-blk
    assert_eq!(pci_config.func.devid, 0x1001);
    log::info!("virtiod: found `virtio-blk` device");

    // Get the PCI capabilities.
    // for vendor_cap in pcid_handle
    //     .fetch_all_features()
    //     .unwrap()
    //     .iter()
    //     .filter(|(x, _)| matches!(x, PciFeature::VendorSpecific))
    // {

    // }
    // log::info!("{}", x.unwrap() as u8);

    loop {}
}
