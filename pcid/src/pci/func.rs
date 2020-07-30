use super::PciDev;
use std::convert::TryFrom;
use syscall::Mmio;

pub trait ConfigReader {
    unsafe fn read_range_into(&self, offset: u16, buf: &mut [u8]) {
        assert!(buf.len() >= 4);
        assert!(buf.len() % 4 == 0);

        let len = u16::try_from(buf.len()).unwrap();

        for (i, dword_off) in (offset..offset + len).step_by(4).enumerate() {
            //if dword_off + 4 > len { break }

            let dword = self.read_u32(dword_off);

            let dword_off = usize::try_from(dword_off).expect("sorry, fellow 8-bit computer :(");
            buf[i * 4..(i + 1) * 4].copy_from_slice(&u32::to_le_bytes(dword));
        }
    }
    unsafe fn read_range(&self, offset: u16, len: u16) -> Vec<u8> {
        let len_usize = usize::try_from(len).expect("sorry, fellow 8-bit computer :(");

        let mut ret = vec![0u8; len_usize];
        self.read_range_into(offset, &mut ret);
        ret
    }

    unsafe fn read_u32(&self, offset: u16) -> u32;

    unsafe fn read_u8(&self, offset: u16) -> u8 {
        let dword_offset = (offset / 4) * 4;
        let dword = self.read_u32(dword_offset);

        let shift = (offset % 4) * 8;
        ((dword >> shift) & 0xFF) as u8
    }
    unsafe fn with_mapped_mem(&self, f: &mut dyn FnMut(Option<&'static mut [Mmio<u32>]>)) {
        f(None);
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
    unsafe fn with_mapped_mem(&self, f: &mut dyn FnMut(Option<&'static mut [Mmio<u32>]>)) {
        self.dev
            .bus
            .pci
            .with_mapped_mem(self.dev.bus.num, self.dev.num, self.num, f);
    }
}
impl<'pci> ConfigWriter for PciFunc<'pci> {
    unsafe fn write_u32(&self, offset: u16, value: u32) {
        self.dev.write(self.num, offset, value);
    }
}
