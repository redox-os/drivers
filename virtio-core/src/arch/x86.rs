pub fn enable_msix(pcid_handle: &mut PcidServerHandle) -> Result<File, Error> {
    panic!("virtio-core: x86 doesn't support enable_msix")
}

pub fn probe_legacy_port_transport<'a>(
    pci_header: &PciHeader,
    pcid_handle: &mut PcidServerHandle,
) -> Result<Device<'a>, Error> {
    crate::x86_64::probe_legacy_port_transport(pci_header, pcid_handle)
}