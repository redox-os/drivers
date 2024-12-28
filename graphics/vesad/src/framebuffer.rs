use std::ptr;

pub struct FrameBuffer {
    pub onscreen: *mut [u32],
    pub phys: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl FrameBuffer {
    pub unsafe fn new(phys: usize, width: usize, height: usize, stride: usize) -> Self {
        let size = stride * height;
        let virt = common::physmap(
            phys,
            size * 4,
            common::Prot {
                read: true,
                write: true,
            },
            common::MemoryType::WriteCombining,
        )
        .expect("vesad: failed to map framebuffer") as *mut u32;

        let onscreen = ptr::slice_from_raw_parts_mut(virt, size);

        Self {
            onscreen,
            phys,
            width,
            height,
            stride,
        }
    }

    pub unsafe fn parse(var: &str) -> Option<Self> {
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
}
