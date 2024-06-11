use pci_types::{ConfigRegionAccess, PciAddress};

pub struct PciFunc<'pci> {
    pub pci: &'pci dyn ConfigRegionAccess,
    pub addr: PciAddress,
}
