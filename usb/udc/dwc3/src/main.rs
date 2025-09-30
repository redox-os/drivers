pub mod device;

fn main() {
    common::setup_logging(
        "usb",
        "udc",
        "dwc3",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );
}
