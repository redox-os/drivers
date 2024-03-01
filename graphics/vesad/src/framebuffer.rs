use std::{
    ptr,
    slice
};

pub struct FrameBuffer {
    pub phys: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl FrameBuffer {
    pub fn new(phys: usize, width: usize, height: usize, stride: usize) -> Self {
        Self {
            phys,
            width,
            height,
            stride,
        }
    }

    pub fn parse(var: &str) -> Option<Self> {
        fn parse_number(part: &str) -> Option<usize> {
            let (start, radix) = if part.starts_with("0x") {
                (2, 16)
            } else {
                (0, 10)
            };
            match usize::from_str_radix(&part[start..], radix) {
                Ok(ok) => Some(ok),
                Err(err) => {
                    eprintln!("vesad: failed to parse '{}': {}", part, err);
                    None
                }
            }
        }

        let mut parts = var.split(',');
        let phys = parse_number(parts.next()?)?;
        let width = parse_number(parts.next()?)?;
        let height = parse_number(parts.next()?)?;
        let stride = parse_number(parts.next()?)?;
        Some(Self::new(phys, width, height, stride))
    }

    pub unsafe fn map(&mut self) -> syscall::Result<&'static mut [u32]> {
        let size = self.stride * self.height;
        let virt = common::physmap(
            self.phys,
            size * 4,
            common::Prot { read: true, write: true },
            common::MemoryType::WriteCombining,
        )? as *mut u32;
        //TODO: should we clear the framebuffer here?
        ptr::write_bytes(virt, 0, size);

        Ok(slice::from_raw_parts_mut(
            virt,
            size
        ))
    }
}
