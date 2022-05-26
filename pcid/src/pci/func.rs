use super::PciDev;

pub trait ConfigReader {
    unsafe fn read_range(&self, offset: u16, len: u16) -> Vec<u8> {
        assert!(len > 3 && len % 4 == 0, "invalid range length: {}", len);

        (offset..offset + len).step_by(4).flat_map(|offset| {
            u32::to_le_bytes(self.read_u32(offset))
        }).collect::<Vec<u8>>()
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
    pub dev: &'pci PciDev<'pci>,
    pub num: u8,
}

impl<'pci> ConfigReader for PciFunc<'pci> {
    unsafe fn read_u32(&self, offset: u16) -> u32 {
        self.dev.read(self.num, offset)
    }
}
impl<'pci> ConfigWriter for PciFunc<'pci> {
    unsafe fn write_u32(&self, offset: u16, value: u32) {
        self.dev.write(self.num, offset, value);
    }
}
