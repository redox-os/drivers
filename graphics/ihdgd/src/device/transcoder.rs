use common::io::{Io, MmioPtr};
use syscall::error::Result;

use super::{MmioRegion, Pipe};

// IHD-OS-TGL-Vol 2c-12.21 TRANS_CLK_SEL
pub const TRANS_CLK_SEL_DDI_SHIFT: u32 = 28;

// IHD-OS-TGL-Vol 2c-12.21 TRANS_CONF
pub const TRANS_CONF_ENABLE: u32 = 1 << 31;
pub const TRANS_CONF_STATE: u32 = 1 << 30;

// IHD-OS-TGL-Vol 2c-12.21 TRANS_DDI_FUNC_CTL
pub const TRANS_DDI_FUNC_CTL_ENABLE: u32 = 1 << 31;
pub const TRANS_DDI_FUNC_CTL_DDI_SHIFT: u32 = 27;
pub const TRANS_DDI_FUNC_CTL_MODE_HDMI: u32 = 0b000 << 24;
pub const TRANS_DDI_FUNC_CTL_MODE_DVI: u32 = 0b001 << 24;
pub const TRANS_DDI_FUNC_CTL_MODE_DP_SST: u32 = 0b010 << 24;
pub const TRANS_DDI_FUNC_CTL_MODE_DP_MST: u32 = 0b011 << 24;
pub const TRANS_DDI_FUNC_CTL_BPC_8: u32 = 0b000 << 20;
pub const TRANS_DDI_FUNC_CTL_BPC_10: u32 = 0b001 << 20;
pub const TRANS_DDI_FUNC_CTL_BPC_6: u32 = 0b010 << 20;
pub const TRANS_DDI_FUNC_CTL_BPC_12: u32 = 0b011 << 20;
pub const TRANS_DDI_FUNC_CTL_SYNC_POLARITY_LOW: u32 = 0b00 << 16;
pub const TRANS_DDI_FUNC_CTL_SYNC_POLARITY_VSLOW_HSHIGH: u32 = 0b01 << 16;
pub const TRANS_DDI_FUNC_CTL_SYNC_POLARITY_VSHIGH_HSLOW: u32 = 0b10 << 16;
pub const TRANS_DDI_FUNC_CTL_SYNC_POLARITY_HIGH: u32 = 0b11 << 16;
pub const TRANS_DDI_FUNC_CTL_PIPE_SHIFT: u32 = 12;

pub struct Transcoder {
    pub name: &'static str,
    pub index: usize,
    pub clk_sel: MmioPtr<u32>,
    pub conf: MmioPtr<u32>,
    pub ddi_func_ctl: MmioPtr<u32>,
    pub ddi_func_ctl2: MmioPtr<u32>,
    pub hblank: MmioPtr<u32>,
    pub hsync: MmioPtr<u32>,
    pub htotal: MmioPtr<u32>,
    pub msa_misc: MmioPtr<u32>,
    pub mult: MmioPtr<u32>,
    pub push: MmioPtr<u32>,
    pub space: MmioPtr<u32>,
    pub stereo3d_ctl: MmioPtr<u32>,
    pub vblank: MmioPtr<u32>,
    pub vrr_ctl: MmioPtr<u32>,
    pub vrr_flipline: MmioPtr<u32>,
    pub vrr_status: MmioPtr<u32>,
    pub vrr_status2: MmioPtr<u32>,
    pub vrr_vmax: MmioPtr<u32>,
    pub vrr_vmaxshift: MmioPtr<u32>,
    pub vrr_vmin: MmioPtr<u32>,
    pub vrr_vtotal_prev: MmioPtr<u32>,
    pub vsync: MmioPtr<u32>,
    pub vsyncshift: MmioPtr<u32>,
    pub vtotal: MmioPtr<u32>,
}

impl Transcoder {
    pub fn dump(&self) {
        eprint!("Transcoder {} {}", self.name, self.index);
        eprint!(" clk_sel {:08X}", self.clk_sel.read());
        eprint!(" conf {:08X}", self.conf.read());
        eprint!(" ddi_func_ctl {:08X}", self.ddi_func_ctl.read());
        eprint!(" ddi_func_ctl2 {:08X}", self.ddi_func_ctl2.read());
        eprint!(" hblank {:08X}", self.hblank.read());
        eprint!(" hsync {:08X}", self.hsync.read());
        eprint!(" htotal {:08X}", self.htotal.read());
        eprint!(" msa_misc {:08X}", self.msa_misc.read());
        eprint!(" mult {:08X}", self.mult.read());
        eprint!(" push {:08X}", self.push.read());
        eprint!(" space {:08X}", self.space.read());
        eprint!(" stereo3d_ctl {:08X}", self.stereo3d_ctl.read());
        eprint!(" vblank {:08X}", self.vblank.read());
        eprint!(" vrr_ctl {:08X}", self.vrr_ctl.read());
        eprint!(" vrr_flipline {:08X}", self.vrr_flipline.read());
        eprint!(" vrr_status {:08X}", self.vrr_status.read());
        eprint!(" vrr_status2 {:08X}", self.vrr_status2.read());
        eprint!(" vrr_vmax {:08X}", self.vrr_vmax.read());
        eprint!(" vrr_vmaxshift {:08X}", self.vrr_vmaxshift.read());
        eprint!(" vrr_vmin {:08X}", self.vrr_vmin.read());
        eprint!(" vrr_vtotal_prev {:08X}", self.vrr_vtotal_prev.read());
        eprint!(" vsync {:08X}", self.vsync.read());
        eprint!(" vsyncshift {:08X}", self.vsyncshift.read());
        eprint!(" vtotal {:08X}", self.vtotal.read());
        eprintln!();
    }

    pub fn modeset(&mut self, pipe: &mut Pipe, timing: &edid::DetailedTiming) {
        let hactive = (timing.horizontal_active_pixels as u32) - 1;
        let htotal = hactive + (timing.horizontal_blanking_pixels as u32);
        let hsync_start = hactive + (timing.horizontal_front_porch as u32);
        let hsync_end = hsync_start + (timing.horizontal_sync_width as u32);
        let vactive = (timing.vertical_active_lines as u32) - 1;
        let vtotal = vactive + (timing.vertical_blanking_lines as u32);
        let vsync_start = vactive + (timing.vertical_front_porch as u32);
        let vsync_end = vsync_start + (timing.vertical_sync_width as u32);

        // Configure horizontal sync
        self.htotal.write(hactive | (htotal << 16));
        self.hblank.write(hactive | (htotal << 16));
        self.hsync.write(hsync_start | (hsync_end << 16));

        // Configure vertical sync
        //TODO: causes reset: self.vtotal.write(vactive | (vtotal << 16));
        self.vblank.write(vactive | (vtotal << 16));
        self.vsync.write(vsync_start | (vsync_end << 16));

        // Configure pipe
        pipe.srcsz.write(vactive | (hactive << 16));
    }

    pub fn tigerlake(gttmm: &MmioRegion) -> Result<Vec<Self>> {
        let mut transcoders = Vec::with_capacity(4);
        for (i, name) in ["A", "B", "C", "D"].iter().enumerate() {
            transcoders.push(Transcoder {
                name,
                index: i,
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_CLK_SEL
                clk_sel: unsafe { gttmm.mmio(0x46140 + i * 0x4)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_CONF
                conf: unsafe { gttmm.mmio(0x70008 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_DDI_FUNC_CTL
                ddi_func_ctl: unsafe { gttmm.mmio(0x60400 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_DDI_FUNC_CTL2
                ddi_func_ctl2: unsafe { gttmm.mmio(0x60404 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_HBLANK
                hblank: unsafe { gttmm.mmio(0x60004 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_HSYNC
                hsync: unsafe { gttmm.mmio(0x60008 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_HTOTAL
                htotal: unsafe { gttmm.mmio(0x60000 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_MSA_MISC
                msa_misc: unsafe { gttmm.mmio(0x60410 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_MULT
                mult: unsafe { gttmm.mmio(0x6002C + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_PUSH
                push: unsafe { gttmm.mmio(0x60A70 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_SPACE
                space: unsafe { gttmm.mmio(0x60020 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_STEREO3D_CTL
                stereo3d_ctl: unsafe { gttmm.mmio(0x70020 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VBLANK
                vblank: unsafe { gttmm.mmio(0x60010 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_CTL
                vrr_ctl: unsafe { gttmm.mmio(0x60420 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_FLIPLINE
                vrr_flipline: unsafe { gttmm.mmio(0x60438 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_STATUS
                vrr_status: unsafe { gttmm.mmio(0x6042C + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_STATUS2
                vrr_status2: unsafe { gttmm.mmio(0x6043C + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_VMAX
                vrr_vmax: unsafe { gttmm.mmio(0x60424 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_VMAXSHIFT
                vrr_vmaxshift: unsafe { gttmm.mmio(0x60428 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_VMIN
                vrr_vmin: unsafe { gttmm.mmio(0x60434 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VRR_VTOTAL_PREV
                vrr_vtotal_prev: unsafe { gttmm.mmio(0x60480 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VSYNC
                vsync: unsafe { gttmm.mmio(0x60014 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VSYNCSHIFT
                vsyncshift: unsafe { gttmm.mmio(0x60028 + i * 0x1000)? },
                // IHD-OS-TGL-Vol 2c-12.21 TRANS_VTOTAL
                vtotal: unsafe { gttmm.mmio(0x6000C + i * 0x1000)? },
            })
        }
        Ok(transcoders)
    }
}