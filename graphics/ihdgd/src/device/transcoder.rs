use common::io::{Io, Mmio};
use syscall::error::Result;

use super::MmioRegion;

pub struct Transcoder {
    pub name: &'static str,
    pub clk_sel: &'static mut Mmio<u32>,
    pub conf: &'static mut Mmio<u32>,
    pub ddi_func_ctl: &'static mut Mmio<u32>,
    pub ddi_func_ctl2: &'static mut Mmio<u32>,
    pub hblank: &'static mut Mmio<u32>,
    pub hsync: &'static mut Mmio<u32>,
    pub htotal: &'static mut Mmio<u32>,
    pub msa_misc: &'static mut Mmio<u32>,
    pub mult: &'static mut Mmio<u32>,
    pub push: &'static mut Mmio<u32>,
    pub space: &'static mut Mmio<u32>,
    pub stereo3d_ctl: &'static mut Mmio<u32>,
    pub vblank: &'static mut Mmio<u32>,
    pub vrr_ctl: &'static mut Mmio<u32>,
    pub vrr_flipline: &'static mut Mmio<u32>,
    pub vrr_status: &'static mut Mmio<u32>,
    pub vrr_status2: &'static mut Mmio<u32>,
    pub vrr_vmax: &'static mut Mmio<u32>,
    pub vrr_vmaxshift: &'static mut Mmio<u32>,
    pub vrr_vmin: &'static mut Mmio<u32>,
    pub vrr_vtotal_prev: &'static mut Mmio<u32>,
    pub vsync: &'static mut Mmio<u32>,
    pub vsyncshift: &'static mut Mmio<u32>,
    pub vtotal: &'static mut Mmio<u32>,
}

impl Transcoder {
    pub fn dump(&self) {
        eprint!("Transcoder {}", self.name);
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

    pub fn tigerlake(gttmm: &MmioRegion) -> Result<Vec<Self>> {
        let mut transcoders = Vec::with_capacity(4);
        for (i, name) in ["A", "B", "C", "D"].iter().enumerate() {
            transcoders.push(Transcoder {
                name,
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