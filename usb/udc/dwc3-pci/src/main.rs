pub mod pci;

use pcid_interface::PciFunctionHandle;
use dwc3::dwc3_init;

fn main() {
    common::setup_logging(
        "pci",
        "udc",
        "dwc3",
        log::LevelFilter::Info,
        log::LevelFilter::Info,
    );

    let mut pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();
    
    let mut name = pci_config.func.name();
    name.push_str("_dwc3");

    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("dwc3-pci: no legacy interrupts supported");
    eprintln!(" + dwc3-pci {}", pci_config.func.display());

    redox_daemon::Daemon::new(move |daemon| {
        let address = unsafe { pcid_handle.map_bar(0) }.ptr.as_ptr() as usize;
        let scheme = dwc3_init(name, address).unwrap();

        unreachable!();
    })
    .expect("dwc3-pci: failed to create daemon");
}
