use pcid_interface::PciFunctionHandle;

fn pci_main() {
    let pcid_handle = PciFunctionHandle::connect_default();
    let pci_config = pcid_handle.config();
    
    let mut name = pci_config.func.name();
    name.push_str("_dwc3");

    let bar0 = pci_config.func.bars[0].expect_port();
    let irq = pci_config
        .func
        .legacy_interrupt_line
        .expect("dwc3: no legacy interrupts supported");
    eprintln!(" + DWC3 {}", pci_config.func.display());
}
