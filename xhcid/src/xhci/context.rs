use std::collections::BTreeMap;

use common::io::{Io, Mmio};
use log::debug;
use syscall::error::Result;
use syscall::PAGE_SIZE;

use common::dma::Dma;

use super::ring::Ring;
use super::Xhci;

#[repr(packed)]
pub struct SlotContext {
    pub a: Mmio<u32>,
    pub b: Mmio<u32>,
    pub c: Mmio<u32>,
    pub d: Mmio<u32>,
    _rsvd: [Mmio<u32>; 4],
}

pub const SLOT_CONTEXT_STATE_MASK: u32 = 0xF800_0000;
pub const SLOT_CONTEXT_STATE_SHIFT: u8 = 27;

impl SlotContext {
    pub fn state(&self) -> u8 {
        ((self.d.read() & SLOT_CONTEXT_STATE_MASK) >> SLOT_CONTEXT_STATE_SHIFT) as u8
    }
}

#[repr(u8)]
pub enum SlotState {
    EnabledOrDisabled = 0,
    Default = 1,
    Addressed = 2,
    Configured = 3,
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

pub const ENDPOINT_CONTEXT_STATUS_MASK: u32 = 0x7;

#[repr(packed)]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

#[repr(packed)]
pub struct InputContext {
    pub drop_context: Mmio<u32>,
    pub add_context: Mmio<u32>,
    _rsvd: [Mmio<u32>; 5],
    pub control: Mmio<u32>,
    pub device: DeviceContext,
}
impl InputContext {
    pub fn dump_control(&self) {
        debug!(
            "INPUT CONTEXT: {} {} [{} {} {} {} {}] {}",
            self.drop_context.read(),
            self.add_context.read(),
            self._rsvd[0].read(),
            self._rsvd[1].read(),
            self._rsvd[2].read(),
            self._rsvd[3].read(),
            self._rsvd[4].read(),
            self.control.read()
        );
    }
}

pub struct DeviceContextList {
    pub dcbaa: Dma<[u64; 256]>,
    pub contexts: Box<[Dma<DeviceContext>]>,
}

impl DeviceContextList {
    pub fn new(ac64: bool, max_slots: u8) -> Result<DeviceContextList> {
        let mut dcbaa = unsafe { Xhci::alloc_dma_zeroed_raw::<[u64; 256]>(ac64)? };
        let mut contexts = vec![];

        // Create device context buffers for each slot
        for i in 0..max_slots as usize {
            let context: Dma<DeviceContext> = unsafe { Xhci::alloc_dma_zeroed_raw(ac64) }?;
            dcbaa[i] = context.physical() as u64;
            contexts.push(context);
        }

        Ok(DeviceContextList {
            dcbaa,
            contexts: contexts.into_boxed_slice(),
        })
    }

    pub fn dcbaap(&self) -> u64 {
        self.dcbaa.physical() as u64
    }
}

#[repr(packed)]
pub struct StreamContext {
    trl: Mmio<u32>,
    trh: Mmio<u32>,
    edtla: Mmio<u32>,
    rsvd: Mmio<u32>,
}

unsafe impl plain::Plain for StreamContext {}

#[repr(u8)]
pub enum StreamContextType {
    SecondaryRing,
    PrimaryRing,
    PrimarySsa8,
    PrimarySsa16,
    PrimarySsa32,
    PrimarySsa64,
    PrimarySsa128,
    PrimarySsa256,
}

pub struct StreamContextArray {
    pub contexts: Dma<[StreamContext]>,
    pub rings: BTreeMap<u16, Ring>,
}

impl StreamContextArray {
    pub fn new(ac64: bool, count: usize) -> Result<Self> {
        unsafe {
            Ok(Self {
                contexts: Xhci::alloc_dma_zeroed_unsized_raw(ac64, count)?,
                rings: BTreeMap::new(),
            })
        }
    }
    pub fn add_ring(&mut self, ac64: bool, stream_id: u16, link: bool) -> Result<()> {
        // NOTE: stream_id 0 is reserved
        assert_ne!(stream_id, 0);

        let ring = Ring::new(ac64, 16, link)?;
        let pointer = ring.register();
        let sct = StreamContextType::PrimaryRing;

        assert_eq!(pointer & (!0xE), pointer);
        {
            let context = &mut self.contexts[stream_id as usize];
            context.trl.write((pointer as u32) | ((sct as u32) << 1));
            context.trh.write((pointer >> 32) as u32);
            // TODO: stopped edtla
        }
        self.rings.insert(stream_id, ring);
        Ok(())
    }
    pub fn register(&self) -> u64 {
        self.contexts.physical() as u64
    }
}

#[repr(packed)]
pub struct ScratchpadBufferEntry {
    pub value_low: Mmio<u32>,
    pub value_high: Mmio<u32>,
}
impl ScratchpadBufferEntry {
    pub fn set_addr(&mut self, addr: u64) {
        self.value_low.write(addr as u32);
        self.value_high.write((addr >> 32) as u32);
    }
}

pub struct ScratchpadBufferArray {
    pub entries: Dma<[ScratchpadBufferEntry]>,
    pub pages: Vec<Dma<[u8; PAGE_SIZE]>>,
}
impl ScratchpadBufferArray {
    pub fn new(ac64: bool, entries: u16) -> Result<Self> {
        let mut entries = unsafe { Xhci::alloc_dma_zeroed_unsized_raw(ac64, entries as usize)? };

        let pages = entries
            .iter_mut()
            .map(
                |entry: &mut ScratchpadBufferEntry| -> Result<_, syscall::Error> {
                    let dma = unsafe { Dma::<[u8; PAGE_SIZE]>::zeroed()?.assume_init() };
                    assert_eq!(dma.physical() % PAGE_SIZE, 0);
                    entry.set_addr(dma.physical() as u64);
                    Ok(dma)
                },
            )
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { entries, pages })
    }
    pub fn register(&self) -> usize {
        self.entries.physical()
    }
}
