//! Interface to `pcid`.

pub mod driver_interface;
pub mod pci;
pub mod pcie;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PciAddr {
    pub seg: u16,
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}
#[derive(Debug)]
pub struct Malformed;

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed PCI address, expected AAAA.BB.CC.DD or BB.CC.DD")
    }
}
impl std::error::Error for Malformed {}

impl std::str::FromStr for PciAddr {
    type Err = Malformed;

    fn from_str(addr: &str) -> Result<Self, Malformed> {
        parse_pci_addr(addr).ok_or(Malformed)
    }
}
fn parse_pci_addr(addr: &str) -> Option<PciAddr> {
    let mut numbers = addr.split('.');

    Some(PciAddr {
        func: numbers.next_back().and_then(|n| u8::from_str_radix(n, 16).ok())?,
        dev: numbers.next_back().and_then(|n| u8::from_str_radix(n, 16).ok())?,
        bus: numbers.next_back().and_then(|n| u8::from_str_radix(n, 16).ok())?,
        seg: {
            let seg = u16::from_str_radix(numbers.next_back().unwrap_or("0"), 16).ok()?;
            if numbers.next_back().is_some() { return None; }
            seg
        }
    })
}
impl std::fmt::Display for PciAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.seg != 0 { write!(f, "{:>04X}", self.seg)?; }
        write!(f, "{:>02X}.{:>02X}.{:>02X}", self.bus, self.dev, self.func)
    }
}
