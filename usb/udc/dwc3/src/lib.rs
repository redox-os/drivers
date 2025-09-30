pub mod device;

extern crate alloc;

use driver_udc::{UDCAdapter, UDCScheme};
use syscall::{
    Error, Result, Stat, EACCES, EAGAIN, EBADF, EINTR, EINVAL, EWOULDBLOCK, MODE_FILE,
};

use core::ptr::NonNull;

use alloc::alloc::{alloc_zeroed, Layout};


pub struct DWC3 {
    address: usize,

    event_buf: Option<NonNull<DWC3EventBuffer>>,

    num_usb3_ports: u32, 
    num_usb2_ports: u32,
}

struct DWC3EventBuffer {
    buf: Option<NonNull<u8>>,
    cache: Option<NonNull<u8>>,
    length: usize, 
    lpos: u32,
    count: u32,
    flags: u32,
    dma: usize,
}

impl DWC3 {
    pub fn new(address: usize) -> Result<Self> {
        let mut dwc3 = unsafe { Self::probe(address)? };
        unsafe {
            dwc3.init();
        }

        Ok(dwc3)
    }

    unsafe fn probe(address: usize) -> Result<Self> {
        // todo: detect host mode ports
        let num_usb3_ports = 1;
        let num_usb2_ports = 1;

        let mut dwc3 = Self {
            address,

            event_buf: None,

            num_usb3_ports,
            num_usb2_ports,
        };

        dwc3.alloc_event_buffers(Self::EVENT_BUFFERS_SIZE as usize)?;

        Ok(dwc3)
    }

    unsafe fn alloc_event_buffers(&mut self, size: usize) -> Result<()> {
        /* todo: check if mode is host */

        let layout = Layout::new::<DWC3EventBuffer>();

        self.event_buf = NonNull::new(alloc_zeroed(layout) as *mut DWC3EventBuffer);

        let layout = Layout::from_size_align(size, 1).unwrap();

        /* allocate the cache */
        self.event_buf.unwrap().as_mut().cache = NonNull::new(alloc_zeroed(layout) as *mut u8);

        /* allocate the dma buffer */
        self.event_buf.unwrap().as_mut().buf = NonNull::new(/*todo :dma memory*/0 as *mut u8);

        self.event_buf.unwrap().as_mut().length = size;


        Ok(())
    }

    unsafe fn init(&mut self) {
        self.writel(Self::GUID, 0xdeadbeef);

        self.phy_setup().expect("dwc: failed to run phy_setup()");
        self.phy_init().expect("dwc: failed to run phy_init()");

        /* todo: ulpi,phys_ready */

        self.soft_reset().expect("dwc: failed to run soft_reset()");
        self.setup_global_control().expect("dwc: failed to run setup_global_control()");

        /* todo: num_eps */

        self.phy_power_on().expect("dwc: failed to run phy_power_on()");
        
        self.event_buffers_setup().expect("dwc: failed to run event_buffers_setup()");

    }

    unsafe fn phy_setup(&mut self) -> Result<()> {
	for i in 0u32..self.num_usb3_ports {
            self.ss_phy_setup(i)?;
	}

	for i in 0u32..self.num_usb2_ports {
            self.hs_phy_setup(i)?;
	}

        Ok(())
    }

    unsafe fn phy_init(&mut self) -> Result<()> {
	for i in 0u32..self.num_usb3_ports {
            self.ss_phy_init(i)?;
	}

	for i in 0u32..self.num_usb2_ports {
            self.hs_phy_init(i)?;
	}

        Ok(())
    }

    unsafe fn phy_power_on(&mut self) -> Result<()> {

        Ok(())
    }

    unsafe fn soft_reset(&mut self) -> Result<()> {

        Ok(())
    }

    unsafe fn event_buffers_setup(&mut self) -> Result<()> {
        if let Some(event_buf) = self.event_buf {
            self.writel(Self::gevntadrlo(0), (event_buf.as_ref().dma & 0xFFFF_FFFF) as u32);
            self.writel(Self::gevntadrhi(0), ((event_buf.as_ref().dma >> 32) & 0xFFFF_FFFF) as u32);
            self.writel(Self::gevntsiz(0), (event_buf.as_ref().length & 0xFFFF_FFFF) as u32);

            self.writel(Self::gevntcount(0), self.readl(Self::gevntcount(0)));
        }

        Ok(())
    }

    unsafe fn setup_global_control(&mut self) -> Result<()> {
        let mut reg: u32;
        reg = self.readl(Self::GCTL);
        reg &= !Self::GCTL_SCALEDOWN_MASK;

        /* todo: get hw_mode and power_opt */

        self.writel(Self::GCTL, reg);

        Ok(())
    }

    unsafe fn ss_phy_setup(&mut self, index: u32) -> Result<()> {
	let mut reg: u32;

	reg = self.readl(Self::gusb3pipectl(index));

        /*
	 * Make sure UX_EXIT_PX is cleared as that causes issues with some
	 * PHYs. Also, this bit is not supposed to be used in normal operation.
	 */
        reg &= !Self::GUSB3PIPECTL_UX_EXIT_PX;

	/* Ensure the GUSB3PIPECTL.SUSPENDENABLE is cleared prior to phy init. */
	reg &= !Self::GUSB3PIPECTL_SUSPHY;

	self.writel(Self::gusb3pipectl(index), reg);

        Ok(())
    }

    unsafe fn ss_phy_init(&mut self, index: u32) -> Result<()> {
        /* todo: nothing to do for pcie, to be implemented for SoC */
        Ok(())
    }

    fn hs_phy_setup(&mut self, index: u32) -> Result<()> {
        /* todo: implement */
        Ok(())
    }

    unsafe fn hs_phy_init(&mut self, index: u32) -> Result<()> {
        /* todo: nothing to do for pcie, to be implemented for SoC */
        Ok(())
    }

    unsafe fn readl(&self, offset: u32) -> u32 {
        *((self.address + offset as usize) as *const u32)
    }

    unsafe fn writel(&mut self, offset: u32, val: u32) {
        *((self.address + offset as usize) as *mut u32) = val;
    }

    const fn gctl_scaledown(n: u32) -> u32 {
        n << 4
    }

    const fn gusb3pipectl(n: u32) -> u32 {
        0xc2c0 + n * 0x04
    }

    const fn gevntadrlo(n: u32) -> u32 {
        0xc400 + ((n) * 0x10)
    }

    const fn gevntadrhi(n: u32) -> u32 {
        0xc404 + ((n) * 0x10)
    }

    const fn gevntsiz(n: u32) -> u32 {
        0xc408 + ((n) * 0x10)
    }

    const fn gevntcount(n: u32) -> u32 {
        0xc40c + ((n) * 0x10)
    }

    const GCTL_SCALEDOWN_MASK: u32 = Self::gctl_scaledown(3);

    const XHCI_REGS_START: u32 = 0x0;
    const XHCI_REGS_END: u32 = 0x7fff;
    const GLOBALS_REGS_START: u32 = 0xc100;
    const GLOBALS_REGS_END: u32 = 0xc6ff;
    const DEVICE_REGS_START: u32 = 0xc700;
    const DEVICE_REGS_END: u32 = 0xcbff;
    const OTG_REGS_START: u32 = 0xcc00;
    const OTG_REGS_END: u32 = 0xccff;

    const GSBUSCFG0: u32 = 0xc100;
    const GSBUSCFG1: u32 = 0xc104;
    const GTXTHRCFG: u32 = 0xc108;
    const GRXTHRCFG: u32 = 0xc10c;
    const GCTL: u32 = 0xc110;
    const GEVTEN: u32 = 0xc114;
    const GSTS: u32 = 0xc118;
    const GUCTL1: u32 = 0xc11c;
    const GSNPSID: u32 = 0xc120;
    const GGPIO: u32 = 0xc124;
    const GUID: u32 = 0xc128;
    const GUCTL: u32 = 0xc12c;
    const GBUSERRADDR0: u32 = 0xc130;
    const GBUSERRADDR1: u32 = 0xc134;
    const GPRTBIMAP0: u32 = 0xc138;
    const GPRTBIMAP1: u32 = 0xc13c;
    const GHWPARAMS0: u32 = 0xc140;
    const GHWPARAMS1: u32 = 0xc144;
    const GHWPARAMS2: u32 = 0xc148;
    const GHWPARAMS3: u32 = 0xc14c;
    const GHWPARAMS4: u32 = 0xc150;
    const GHWPARAMS5: u32 = 0xc154;
    const GHWPARAMS6: u32 = 0xc158;
    const GHWPARAMS7: u32 = 0xc15c;
    const GDBGFIFOSPACE: u32 = 0xc160;
    const GDBGLTSSM: u32 = 0xc164;
    const GDBGBMU: u32 = 0xc16c;
    const GDBGLSPMUX: u32 = 0xc170;
    const GDBGLSP: u32 = 0xc174;
    const GDBGEPINFO0: u32 = 0xc178;
    const GDBGEPINFO1: u32 = 0xc17c;
    const GPRTBIMAP_HS0: u32 = 0xc180;
    const GPRTBIMAP_HS1: u32 = 0xc184;
    const GPRTBIMAP_FS0: u32 = 0xc188;
    const GPRTBIMAP_FS1: u32 = 0xc18c;
    const GUCTL2: u32 = 0xc19c;

    const GUSB3PIPECTL_UX_EXIT_PX: u32 = 1 << 27;
    const GUSB3PIPECTL_SUSPHY: u32 = 1 << 17;

    const EVENT_BUFFERS_SIZE: u32 = 4096;
}

impl UDCAdapter for DWC3 {
    fn write_ep(&mut self, ep: usize, buf: &[u8]) -> Result<usize> {
        todo!("Todo");
    }

    fn read_ep(&mut self, ep: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        todo!("Todo");
    }
}


