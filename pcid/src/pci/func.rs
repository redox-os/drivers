use byteorder::{ByteOrder, LittleEndian};
use pci_types::{ConfigRegionAccess, PciAddress};

pub trait ConfigReader {
    unsafe fn read_range(&self, offset: u16, len: u16) -> Vec<u8> {
        assert!(len > 3 && len % 4 == 0, "invalid range length: {}", len);
        let mut ret = Vec::with_capacity(len as usize);
        let results = (offset..offset + len)
            .step_by(4)
            .fold(Vec::new(), |mut acc, offset| {
                let val = self.read_u32(offset);
                acc.push(val);
                acc
            });
        ret.set_len(len as usize);
        LittleEndian::write_u32_into(&*results, &mut ret);
        ret
    }

    unsafe fn read_u32(&self, offset: u16) -> u32;

    unsafe fn read_u8(&self, offset: u16) -> u8 {
        let dword_offset = (offset / 4) * 4;
        let dword = self.read_u32(dword_offset);

        let shift = (offset % 4) * 8;
        ((dword >> shift) & 0xFF) as u8
    }
}
pub trait ConfigWriter {
    unsafe fn write_u32(&self, offset: u16, value: u32);
}

pub struct PciFunc<'pci> {
    pub pci: &'pci dyn ConfigRegionAccess,
    pub addr: PciAddress,
}

impl<'pci> ConfigReader for PciFunc<'pci> {
    unsafe fn read_u32(&self, offset: u16) -> u32 {
        self.pci.read(self.addr, offset)
    }
}
impl<'pci> ConfigWriter for PciFunc<'pci> {
    unsafe fn write_u32(&self, offset: u16, value: u32) {
        self.pci.write(self.addr, offset, value);
    }
}
