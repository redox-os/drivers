use syscall::error::Result;
use syscall::io::{Dma, Mmio};

#[repr(packed)]
pub struct SlotContext {
    pub a: Mmio<u32>,
    pub b: Mmio<u32>,
    pub c: Mmio<u32>,
    pub d: Mmio<u32>,
    _rsvd: [Mmio<u32>; 4],
}

#[repr(packed)]
pub struct EndpointContext {
    pub a: Mmio<u32>,
    pub b: Mmio<u32>,
    pub trl: Mmio<u32>,
    pub trh: Mmio<u32>,
    pub c: Mmio<u32>,
    _rsvd: [Mmio<u32>; 3],
}

#[repr(packed)]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 15]
}

#[repr(packed)]
pub struct InputContext {
    pub drop_context: Mmio<u32>,
    pub add_context: Mmio<u32>,
    _rsvd: [Mmio<u32>; 5],
    pub control: Mmio<u32>,
    pub device: DeviceContext,
}

pub struct DeviceList {
    pub dcbaa: Dma<[u64; 256]>,
    pub contexts: Vec<Dma<DeviceContext>>,
}

impl DeviceList {
    pub fn new(max_slots: u8) -> Result<DeviceList> {
        let mut dcbaa = Dma::<[u64; 256]>::zeroed()?;
        let mut contexts = vec![];

        // Create device context buffers for each slot
        for i in 0..max_slots as usize {
            println!("  - Setup dev ctx {}", i);
            let context: Dma<DeviceContext> = Dma::zeroed()?;
            dcbaa[i] = context.physical() as u64;
            contexts.push(context);
        }

        Ok(DeviceList {
            dcbaa: dcbaa,
            contexts: contexts
        })
    }

    pub fn dcbaap(&self) -> u64 {
        self.dcbaa.physical() as u64
    }
}
