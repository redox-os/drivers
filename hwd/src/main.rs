mod backend;
use self::backend::{AcpiBackend, Backend, DeviceTreeBackend, LegacyBackend};

fn main() {
    common::setup_logging(
        "misc",
        "hwd",
        "hwd",
        common::output_level(),
        common::file_level(),
    );

    // Prefer DTB if available (matches kernel preference)
    let mut backend: Box<dyn Backend> = match DeviceTreeBackend::new() {
        Ok(ok) => {
            log::info!("using devicetree backend");
            Box::new(ok)
        }
        Err(err) => {
            log::debug!("cannot use devicetree backend: {}", err);
            match AcpiBackend::new() {
                Ok(ok) => {
                    log::info!("using ACPI backend");
                    Box::new(ok)
                }
                Err(err) => {
                    log::debug!("cannot use ACPI backend: {}", err);

                    log::info!("using legacy backend");
                    Box::new(LegacyBackend)
                }
            }
        }
    };

    //TODO: HWD is meant to locate PCI/XHCI/etc devices in ACPI and DeviceTree definitions and start their drivers
    match backend.probe() {
        Ok(()) => {}
        Err(err) => {
            log::error!("failed to probe with error {}", err);
        }
    }
}
