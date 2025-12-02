use common::{io::{Io, MmioPtr}, timeout::Timeout};
use pcid_interface::PciFunction;
use std::{mem, ptr, sync::Arc};
use syscall::error::{Error, Result, EIO, ENODEV, ERANGE};

mod ddi;
use self::ddi::*;
mod dpll;
use self::dpll::*;
mod pipe;
use self::pipe::*;
mod transcoder;
use self::transcoder::*;

//TODO: move to common?
pub struct CallbackGuard<'a, T, F: FnOnce(&mut T)> {
    value: &'a mut T,
    fini: Option<F>,
}

impl<'a, T, F: FnOnce(&mut T)> CallbackGuard<'a, T, F> {
    // Note that fini will also run if init fails
    pub fn new(value: &'a mut T, init: impl FnOnce(&mut T) -> Result<()>, fini: F) -> Result<Self> {
        let mut this = Self {
            value,
            fini: Some(fini),
        };
        init(&mut this.value)?;
        Ok(this)
    }
}

impl<'a, T, F: FnOnce(&mut T)> Drop for CallbackGuard<'a, T, F> {
    fn drop(&mut self) {
        let fini = self.fini.take().unwrap();
        fini(&mut self.value);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum DeviceKind {
    TigerLake,
}

#[derive(Debug)]
pub struct MmioRegion {
    phys: usize,
    virt: usize,
    size: usize,
}

impl MmioRegion {
    fn new(phys: usize, size: usize) -> Result<Self> {
        let virt = unsafe {
            common::physmap(
                phys,
                size,
                common::Prot::RW,
                common::MemoryType::Uncacheable,
            )? as usize
        };
        Ok(Self {
            phys,
            virt,
            size,
        })
    }

    unsafe fn mmio(&self, offset: usize) -> Result<MmioPtr<u32>> {
        // Any errors here will return ERANGE
        let err = Error::new(ERANGE);
        if offset.checked_add(mem::size_of::<u32>()).ok_or(err)? > self.size {
            return Err(err);
        }
        let addr = self.virt.checked_add(offset).ok_or(err)?;
        Ok(unsafe { MmioPtr::new(addr as *mut u32) })
    }
}

impl Drop for MmioRegion {
    fn drop(&mut self) {
        unsafe {
            let _ = libredox::call::munmap(self.virt as *mut (), self.size);
        }
    }
}

pub struct Device {
    kind: DeviceKind,
    ddis: Vec<Ddi>,
    dplls: Vec<Dpll>,
    gttmm: Arc<MmioRegion>,
    gm: MmioRegion,
    pipes: Vec<Pipe>,
    transcoders: Vec<Transcoder>,
}

impl Device {
    pub fn new(func: &PciFunction) -> Result<Self> {
        let kind = match (func.full_device_id.vendor_id, func.full_device_id.device_id) {
            (0x8086, 0x9A40) |
            (0x8086, 0x9A49) |
            (0x8086, 0x9A60) |
            (0x8086, 0x9A68) |
            (0x8086, 0x9A70) |
            (0x8086, 0x9A78) => {
                DeviceKind::TigerLake
            }
            (vendor_id, device_id) => {
                log::error!("unsupported ID {:04X}:{:04X}", vendor_id, device_id);
                return Err(Error::new(ENODEV));
            }
        };

        let gttmm = {
            let (phys, size) = func.bars[0].expect_mem();
            Arc::new(MmioRegion::new(phys, size)?)
        };
        log::info!("GTTMM {:X?}", gttmm);
        let gm = {
            let (phys, size) = func.bars[2].expect_mem();
            MmioRegion::new(phys, size)?
        };
        log::info!("GM {:X?}", gttmm);
        /* IOBAR not used
        let iobar = func.bars[4].expect_port();
        log::debug!("IOBAR {:X?}", iobar);
        */

        let de_hpd_interrupt;
        let de_port_interrupt;
        let mut dpclka_cfgcr0;
        let dssm;
        let mut gmbus;
        let mut pwr_well_ctl_aux;
        let mut pwr_well_ctl_ddi;
        let sde_interrupt;
        let shotplug_ctl_ddi;
        let shotplug_ctl_tc;
        let tbt_hotplug_ctl;
        let tc_hotplug_ctl;
        let mut ddis;
        let mut dplls;
        let mut pipes;
        let mut transcoders;
        match kind {
            DeviceKind::TigerLake => {
                // IHD-OS-TGL-Vol 2c-12.21
                let dc_state_en = unsafe { gttmm.mmio(0x45504)? };
                log::debug!("dc_state_en {:08X}", dc_state_en.read());

                de_hpd_interrupt = unsafe { gttmm.mmio(0x44470)? };
                log::debug!("de_hpd_interrupt {:08X}", de_hpd_interrupt.read());

                de_port_interrupt = unsafe { gttmm.mmio(0x44440)? };
                log::debug!("de_port_interrupt {:08X}", de_port_interrupt.read());

                dpclka_cfgcr0 = unsafe { gttmm.mmio(0x164280)? };
                log::info!("dpclka_cfgcr0 {:08X}", dpclka_cfgcr0.read());

                let dpll0_cfgcr0 = unsafe { gttmm.mmio(0x164284)? };
                log::debug!("dpll0_cfgcr0 {:08X}", dpll0_cfgcr0.read());

                let dpll0_cfgcr1 = unsafe { gttmm.mmio(0x164288)? };
                log::debug!("dpll0_cfgcr1 {:08X}", dpll0_cfgcr1.read());

                let dpll0_enable = unsafe { gttmm.mmio(0x46010)? };
                log::debug!("dpll0_enable {:08X}", dpll0_enable.read());

                let dpll1_enable = unsafe { gttmm.mmio(0x46014)? };
                log::debug!("dpll1_enable {:08X}", dpll1_enable.read());

                let dpll4_enable = unsafe { gttmm.mmio(0x46018)? };
                log::debug!("dpll4_enable {:08X}", dpll4_enable.read());

                // IHD-OS-TGL-Vol 2c-12.21 DSSM
                dssm = unsafe { gttmm.mmio(0x51004)? };
                log::debug!("dssm {:08X}", dssm.read());

                let fuse_status = unsafe { gttmm.mmio(0x42000)? };
                log::debug!("fuse_status {:08X}", fuse_status.read());

                gmbus = unsafe { [
                    gttmm.mmio(0xC5100)?,
                    gttmm.mmio(0xC5104)?,
                    gttmm.mmio(0xC5108)?,
                    gttmm.mmio(0xC510C)?,
                    gttmm.mmio(0xC5110)?,
                    gttmm.mmio(0xC5120)?,
                ] };

                let pwr_well_ctl = unsafe { gttmm.mmio(0x45404)? };
                log::debug!("pwr_well_ctl {:08X}", pwr_well_ctl.read());

                pwr_well_ctl_aux = unsafe { gttmm.mmio(0x45444)? };
                log::debug!("pwr_well_ctl_aux {:08X}", pwr_well_ctl_aux.read());

                pwr_well_ctl_ddi = unsafe { gttmm.mmio(0x45454)? };
                log::debug!("pwr_well_ctl_ddi {:08X}", pwr_well_ctl_ddi.read());

                sde_interrupt = unsafe { gttmm.mmio(0xC4000)? };
                log::debug!("sde_interrupt {:08X}", sde_interrupt.read());

                shotplug_ctl_ddi = unsafe { gttmm.mmio(0xC4030)? };
                log::debug!("shotplug_ctl_ddi {:08X}", shotplug_ctl_ddi.read());

                shotplug_ctl_tc = unsafe { gttmm.mmio(0xC4034)? };
                log::debug!("shotplug_ctl_tc {:08X}", shotplug_ctl_tc.read());

                tbt_hotplug_ctl = unsafe { gttmm.mmio(0x44030)? };
                log::debug!("tbt_hotplug_ctl {:08X}", tbt_hotplug_ctl.read());

                tc_hotplug_ctl = unsafe { gttmm.mmio(0x44038)? };
                log::debug!("tc_hotplug_ctl {:08X}", tc_hotplug_ctl.read());

                ddis = Ddi::tigerlake(&gttmm)?;
                dplls = Dpll::tigerlake(&gttmm)?;
                pipes = Pipe::tigerlake(&gttmm)?;
                transcoders = Transcoder::tigerlake(&gttmm)?;
            },
        };

        const DSSM_REF_FREQ_24_MHZ: u32 = 0b000 << 29;
        const DSSM_REF_FREQ_19_2_MHZ: u32 = 0b001 << 29;
        const DSSM_REF_FREQ_38_4_MHZ: u32 = 0b010 << 29;
        const DSSM_REF_FREQ_MASK: u32 = 0b111 << 29;
        let ref_freq: u64 = match dssm.read() & DSSM_REF_FREQ_MASK {
            DSSM_REF_FREQ_24_MHZ => {
                24_000_000
            },
            DSSM_REF_FREQ_19_2_MHZ => {
                19_200_000
            },
            DSSM_REF_FREQ_38_4_MHZ => {
                38_400_000
            },
            unknown => {
                log::error!("unknown DSSM reference frequency {}", unknown);
                return Err(Error::new(EIO));
            }
        };

        for port in ddis.iter_mut() {
            //TODO: init port if needed
            if let Some(port_comp_dw0) = port.port_comp(PortCompReg::Dw0) {
                log::debug!("PORT_COMP_DW0_{}: {:08X}", port.name, port_comp_dw0.read());
            }

            enum I2CData<'a> {
                Read(&'a mut [u8]),
                Write(&'a [u8]),
            }

            let mut aux_i2c_tx = |port: &mut Ddi, mot: bool, addr: u8, mut data: I2CData| -> Result<()> {
                // Write header and data
                let mut header = 0;
                match &data {
                    I2CData::Read(_) => {
                        header |= 1 << 4;
                    },
                    I2CData::Write(_) => ()
                }
                if mot {
                    header |= 1 << 6;
                }
                let mut aux_datas = [0u8; 20];
                let mut aux_data_i = 0;
                aux_datas[aux_data_i] = header;
                aux_data_i += 1;
                //TODO: what is this byte?
                aux_datas[aux_data_i] = 0;
                aux_data_i += 1;
                aux_datas[aux_data_i] = addr;
                aux_data_i += 1;
                match &data {
                    I2CData::Read(buf) => {
                        if !buf.is_empty() {
                            aux_datas[aux_data_i] = (buf.len() - 1) as u8;
                            aux_data_i += 1;
                        }
                    }
                    I2CData::Write(buf) => {
                        if !buf.is_empty() {
                            aux_datas[aux_data_i] = (buf.len() - 1) as u8;
                            aux_data_i += 1;
                            for b in buf.iter() {
                                aux_datas[aux_data_i] = *b;
                                aux_data_i += 1;
                            }
                        }
                    }
                }

                // Write data to registers (big endian, dword access only)
                for (i, chunk) in aux_datas.chunks(4).enumerate() {
                    let mut bytes = [0; 4];
                    bytes[..chunk.len()].copy_from_slice(&chunk);
                    port.aux_datas[i].write(u32::from_be_bytes(bytes));
                }

                let mut v = port.aux_ctl.read();
                // Set length
                v &= !DDI_AUX_CTL_SIZE_MASK;
                v |= (aux_data_i as u32) << DDI_AUX_CTL_SIZE_SHIFT;
                // Set timeout
                v &= !DDI_AUX_CTL_TIMEOUT_MASK;
                v |= DDI_AUX_CTL_TIMEOUT_4000US;
                // Set I/O select to legacy (cleared)
                //TODO: TBT support?
                v &= !DDI_AUX_CTL_IO_SELECT;
                // Start transaction
                v |= DDI_AUX_CTL_BUSY;
                port.aux_ctl.write(v);

                // Wait while busy
                let timeout = Timeout::from_secs(1);
                while port.aux_ctl.readf(DDI_AUX_CTL_BUSY) {
                    timeout.run().map_err(|()| {
                        log::debug!("AUX I2C transaction wait timeout");
                        Error::new(EIO)
                    })?;
                }

                // Read result
                v = port.aux_ctl.read();
                if (v & DDI_AUX_CTL_TIMEOUT_ERROR) != 0 {
                    log::debug!("AUX I2C transaction timeout error");
                    return Err(Error::new(EIO));
                } 
                if (v & DDI_AUX_CTL_RECEIVE_ERROR) != 0 {
                    log::debug!("AUX I2C transaction receive error");
                    return Err(Error::new(EIO));
                } 
                if (v & DDI_AUX_CTL_DONE) == 0 {
                    log::debug!("AUX I2C transaction done not set");
                    return Err(Error::new(EIO));
                }

                // Read data from registers (big endian, dword access only)
                for (i, chunk) in aux_datas.chunks_mut(4).enumerate() {
                    let bytes = port.aux_datas[i].read().to_be_bytes();
                    chunk.copy_from_slice(&bytes[..chunk.len()]);
                }

                aux_data_i = 0;
                let response = aux_datas[aux_data_i];
                if response != 0 {
                    log::debug!("AUX I2C unexpected response {:02X}", response);
                    return Err(Error::new(EIO));
                }
                aux_data_i += 1;
                match &mut data {
                    I2CData::Read(buf) => {
                        if !buf.is_empty() {
                            for b in buf.iter_mut() {
                                *b = aux_datas[aux_data_i];
                                aux_data_i += 1;
                            }
                        }
                    }
                    I2CData::Write(_) => ()
                }

                Ok(())
            };

            let mut aux_read_edid = |port: &mut Ddi| -> Result<[u8; 128]> {
                //TODO: BLOCK TCCOLD?

                let pwr_well_ctl_aux_request = port.pwr_well_ctl_aux_request();
                let pwr_well_ctl_aux_state = port.pwr_well_ctl_aux_state();
                let _pwr_guard = CallbackGuard::new(
                    &mut pwr_well_ctl_aux,
                    |pwr_well_ctl_aux| {
                        // Enable aux power
                        pwr_well_ctl_aux.writef(pwr_well_ctl_aux_request, true);
                        let timeout = Timeout::from_micros(1500);
                        while !pwr_well_ctl_aux.readf(pwr_well_ctl_aux_state) {
                            timeout.run().map_err(|()| {
                                log::debug!("timeout while requesting port {} aux power", port.name);
                                Error::new(EIO)
                            })?;
                        }
                        Ok(())
                    },
                    |pwr_well_ctl_aux| {
                        // Disable aux power
                        pwr_well_ctl_aux.writef(pwr_well_ctl_aux_request, false);
                    }
                )?;

                // Check if device responds
                aux_i2c_tx(port, true, 0x50, I2CData::Write(&[]))?;
                // Write index
                aux_i2c_tx(port, true, 0x50, I2CData::Write(&[0]))?;
                // Read EDID
                //TODO: Could EDID be read in multiple byte transactions?
                let mut edid_data = [0; 128];
                for chunk in edid_data.chunks_mut(1) {
                    aux_i2c_tx(port, true, 0x50, I2CData::Read(chunk))?;
                }
                // Finish transaction
                aux_i2c_tx(port, false, 0x50, I2CData::Read(&mut []))?;

                Ok(edid_data)
            };

            let mut gmbus_i2c_tx = |port: &mut Ddi, addr7: u8, index: u8, mut data: I2CData| -> Result<()> {
                let Some(gmbus_pin_pair) = port.gmbus_pin_pair() else {
                    log::error!("Port {} has no GMBUS pin pair", port.name);
                    return Err(Error::new(EIO));
                };

                const GMBUS1_SW_RDY: u32 = 1 << 30;
                const GMBUS1_CYCLE_STOP: u32 = 1 << 27;
                const GMBUS1_CYCLE_INDEX: u32 = 1 << 26;
                const GMBUS1_CYCLE_WAIT: u32 = 1 << 25;
                const GMBUS1_SIZE_SHIFT: u32 = 16;
                const GMBUS1_INDEX_SHIFT: u32 = 8;

                const GMBUS2_HW_RDY: u32 = 1 << 11;

                // Reset
                gmbus[1].write(0);

                // Start transaction
                gmbus[0].write(gmbus_pin_pair as u32);
                let (addr8, size) = match &data {
                    I2CData::Read(buf) => ((addr7 << 1) | 1, buf.len() as u32),
                    I2CData::Write(buf) => (addr7 << 1, buf.len() as u32),
                };
                if size >= 512 {
                    log::error!("GMBUS transaction size {} too large", size);
                    return Err(Error::new(EIO));
                }
                gmbus[1].write(
                    GMBUS1_SW_RDY |
                    GMBUS1_CYCLE_INDEX |
                    GMBUS1_CYCLE_WAIT |
                    (size << GMBUS1_SIZE_SHIFT) |
                    (index as u32) << GMBUS1_INDEX_SHIFT |
                    (addr8 as u32)
                );

                // Perform transaction
                match &mut data {
                    I2CData::Read(buf) => {
                        for chunk in buf.chunks_mut(4) {
                            {
                                //TODO: ideal timeout for gmbus read?
                                let timeout = Timeout::from_millis(10);
                                while !gmbus[2].readf(GMBUS2_HW_RDY) {
                                    timeout.run().map_err(|()| {
                                        log::debug!("timeout on GMBUS read");
                                        Error::new(EIO)
                                    })?;
                                }
                            }

                            let bytes = gmbus[3].read().to_le_bytes();
                            chunk.copy_from_slice(&bytes[..chunk.len()]);
                        }
                    },
                    I2CData::Write(buf) => {
                        log::warn!("TODO: GMBUS WRITE");
                    }
                }

                // Stop transaction
                gmbus[1].write(GMBUS1_SW_RDY | GMBUS1_CYCLE_STOP);

                Ok(())
            };

            let mut gmbus_read_edid = |port: &mut Ddi| -> Result<[u8; 128]> {
                let mut edid_data = [0; 128];
                gmbus_i2c_tx(port, 0x50, 0x00, I2CData::Read(&mut edid_data))?;
                Ok(edid_data)
            };

            let (source, edid_data) = match aux_read_edid(port) {
                Ok(edid_data) => ("AUX", edid_data),
                Err(err) => {
                    log::debug!("Port {} failed to read EDID from AUX: {}", port.name, err);
                    match gmbus_read_edid(port) {
                        Ok(edid_data) => ("GMBUS", edid_data),
                        Err(err) => {
                            log::debug!("Port {} failed to read EDID from GMBUS: {}", port.name, err);
                            continue;
                        }
                    }
                }
            };

            let edid = match edid::parse(&edid_data).to_full_result() {
                Ok(edid) => {
                    log::info!("Port {} EDID from {}: {:?}", port.name, source, edid);
                    edid
                },
                Err(err) => {
                    log::warn!("Port {} failed to parse EDID from {}: {:?}", port.name, source, err);
                    continue;
                }
            };

            let mut timing_opt = None;
            for desc in edid.descriptors.iter() {
                match desc {
                    edid::Descriptor::DetailedTiming(timing) => {
                        timing_opt = Some(timing);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(timing) = timing_opt else {
                log::warn!("Port {} EDID from {} missing detailed timing", port.name, source);
                continue;
            };

            let mut modeset_hdmi = |port: &mut Ddi| -> Result<()> {
                // IHD-OS-TGL-Vol 12-1.22-Rev2.0 "Sequences for HDMI and DVI"

                // Power wells should already be enabled

                //TODO: Type-C needs aux power enabled and max lanes set
                
                // Enable port PLL without SSC
                {
                    // Find free DPLL
                    let dpll = dplls.iter_mut().find(|dpll| {
                        !dpll.enable.readf(DPLL_ENABLE_ENABLE)
                    }).ok_or_else(|| {
                        log::error!("failed to find free DPLL");
                        Error::new(EIO)
                    })?;

                    // DPLL power guard
                    let mut dpll_enable = unsafe { MmioPtr::new(dpll.enable.as_mut_ptr()) };
                    let dpll_power_guard = CallbackGuard::new(
                        &mut dpll_enable,
                        |dpll_enable| {
                            // Enable DPLL power
                            dpll_enable.writef(DPLL_ENABLE_POWER_ENABLE, true);
                            //TODO: timeout not specified in docs, should be very fast
                            let timeout = Timeout::from_micros(1);
                            while !dpll_enable.readf(DPLL_ENABLE_POWER_STATE) {
                                timeout.run().map_err(|()| {
                                    log::debug!("timeout while enabling DPLL {} power", dpll.name);
                                    Error::new(EIO)
                                })?;
                            }
                            Ok(())
                        },
                        |dpll_enable| {
                            // Disable DPLL power
                            dpll_enable.writef(DPLL_ENABLE_POWER_ENABLE, false);
                        }
                    )?;

                    // Set SSC enable/disable. For HDMI, always disable
                    dpll.ssc.writef(DPLL_SSC_ENABLE, false);

                    // Configure DPLL frequency
                    dpll.set_freq_hdmi(ref_freq, timing)?;

                    //TODO: "Sequence Before Frequency Change"

                    // Enable DPLL
                    //TODO: use guard?
                    {
                        dpll.enable.writef(DPLL_ENABLE_ENABLE, true);
                        let timeout = Timeout::from_micros(50);
                        while !dpll.enable.readf(DPLL_ENABLE_LOCK) {
                            timeout.run().map_err(|()| {
                                log::debug!("timeout while enabling DPLL {}", dpll.name);
                                Error::new(EIO)
                            })?;
                        }
                    }

                    //TODO: "Sequence After Frequency Change"

                    // Update DPLL mapping
                    {
                        const DPCLKA_CFGCR0_CLOCK_MASK: u32 = 0b11;

                        let Some(clock_shift) = port.dpclka_cfgcr0_clock_shift() else {
                            log::warn!("Port {} clock shift not implemented", port.name);
                            return Err(Error::new(EIO));
                        };
                        let mut v = dpclka_cfgcr0.read();
                        v &= !(DPCLKA_CFGCR0_CLOCK_MASK << clock_shift);
                        v |= (dpll.dpclka_cfgcr0_clock_value << clock_shift);
                        dpclka_cfgcr0.write(v);
                    }

                    // Enable DPLL clock (must be done separately from PLL mapping)
                    {
                        let Some(clock_off) = port.dpclka_cfgcr0_clock_off() else {
                            log::warn!("Port {} clock off not implemented", port.name);
                            return Err(Error::new(EIO));
                        };
                        dpclka_cfgcr0.writef(clock_off, false);
                    }

                    // Continue to allow DPLL power
                    mem::forget(dpll_power_guard);
                }

                // Enable IO power
                let pwr_well_ctl_ddi_request = port.pwr_well_ctl_ddi_request();
                let pwr_well_ctl_ddi_state = port.pwr_well_ctl_ddi_state();
                let pwr_guard = CallbackGuard::new(
                    &mut pwr_well_ctl_ddi,
                    |pwr_well_ctl_ddi| {
                        // Enable IO power
                        pwr_well_ctl_ddi.writef(pwr_well_ctl_ddi_request, true);
                        let timeout = Timeout::from_micros(30);
                        while !pwr_well_ctl_ddi.readf(pwr_well_ctl_ddi_state) {
                            timeout.run().map_err(|()| {
                                log::debug!("timeout while requesting port {} IO power", port.name);
                                Error::new(EIO)
                            })?;
                        }
                        Ok(())
                    },
                    |pwr_well_ctl_ddi| {
                        // Disable IO power
                        pwr_well_ctl_ddi.writef(pwr_well_ctl_ddi_request, false);
                    }
                )?;

                //TODO: Type-C DP_MODE

                // Enable planes, pipe, and transcoder
                {
                    // Find free transcoder with free pipe
                    let mut transcoder_pipe = None;
                    for (transcoder, pipe) in transcoders.iter_mut().zip(pipes.iter_mut()) {
                        if transcoder.conf.readf(TRANS_CONF_ENABLE) {
                            continue;
                        }
                        //TODO: how would we know if pipe is in use?
                        transcoder_pipe = Some((transcoder, pipe));
                        break;
                    }
                    let Some((transcoder, pipe)) = transcoder_pipe else {
                        log::error!("free transcoder and pipe not found");
                        return Err(Error::new(EIO));
                    };

                    // Configure transcoder clock select
                    transcoder.clk_sel.write(port.transcoder_index() << TRANS_CLK_SEL_DDI_SHIFT);

                    //TODO: Configure and enable planes 

                    //TODO: VGA and panel fitter steps?

                    // Configure transcoder timings and other pipe and transcoder settings
                    transcoder.modeset(pipe, timing);

                    // Configure and enable TRANS_DDI_FUNC_CTL
                    {
                        let mut ddi_func_ctl = 
                            TRANS_DDI_FUNC_CTL_ENABLE |
                            (port.transcoder_index() << TRANS_DDI_FUNC_CTL_DDI_SHIFT) |
                            TRANS_DDI_FUNC_CTL_MODE_HDMI |
                            //TODO: allow different bits per color
                            TRANS_DDI_FUNC_CTL_BPC_8 |
                            //TODO: correct port width selection
                            TRANS_DDI_FUNC_CTL_PORT_WIDTH_4;
                        
                        match (timing.features >> 3) & 0b11 {
                            // Digital sync, separate
                            0b11 => {
                                if (timing.features & (1 << 2)) != 0 {
                                    ddi_func_ctl |= TRANS_DDI_FUNC_CTL_SYNC_POLARITY_VSHIGH;
                                }
                                if (timing.features & (1 << 1)) != 0 {
                                    ddi_func_ctl |= TRANS_DDI_FUNC_CTL_SYNC_POLARITY_HSHIGH;
                                }
                            },
                            unsupported => {
                                log::warn!("unsupported sync {:#x}", unsupported);
                            }
                        }

                        // Set scrambling and high TMDS char rate based on symbol rate > 340 MHz
                        if timing.pixel_clock > 340_000 {
                            ddi_func_ctl |= 
                                TRANS_DDI_FUNC_CTL_HIGH_TMDS_CHAR_RATE |
                                TRANS_DDI_FUNC_CTL_HDMI_SCRAMBLING;
                        }

                        transcoder.ddi_func_ctl.write(ddi_func_ctl);
                    }

                    // Configure and enable TRANS_CONF
                    let mut conf = transcoder.conf.read();
                    // Set mode to progressive
                    conf &= !TRANS_CONF_MODE_MASK;
                    // Enable transcoder
                    conf |= TRANS_CONF_ENABLE;
                    transcoder.conf.write(conf);
                    //TODO: what is the correct timeout?
                    let timeout = Timeout::from_millis(100);
                    while !transcoder.conf.readf(TRANS_CONF_STATE) {
                        timeout.run().map_err(|()| {
                            log::error!("timeout on port {} transcoder {} enable", port.name, transcoder.name);
                            Error::new(EIO)
                        })?;
                    }
                }

                // Enable port
                {
                    // Configure voltage swing and related IO settings
                    port.voltage_swing_hdmi(&gttmm, timing)?;

                    // Configure PORT_CL_DW10 static power down to power up all lanes
                    //TODO: only power up required lanes
                    if let Some(mut port_cl_dw10) = port.port_cl(PortClReg::Dw10) {
                        port_cl_dw10.writef(0b1111 << 4, false);
                    }

                    // Configure and enable DDI_BUF_CTL
                    //TODO: more DDI_BUF_CTL bits?
                    port.buf_ctl.writef(DDI_BUF_CTL_ENABLE, true);

                    // Wait for DDI_BUF_CTL IDLE = 0, timeout after 500 us
                    let timeout = Timeout::from_micros(500);
                    while port.buf_ctl.readf(DDI_BUF_CTL_IDLE) {
                        timeout.run().map_err(|()| {
                            log::warn!("timeout while waiting for port {} DDI active", port.name);
                            Error::new(EIO)
                        })?;
                    }
                }

                // Keep IO power on if finished
                mem::forget(pwr_guard);

                Ok(())
            };

            if port.buf_ctl.readf(DDI_BUF_CTL_IDLE) {
                log::info!("Port {} DDI idle, will attempt mode setting", port.name);
                //TODO: DisplayPort modeset
                match modeset_hdmi(port) {
                    Ok(()) => {
                        log::info!("Port {} modeset finished", port.name);
                    },
                    Err(err) => {
                        log::warn!("Port {} modeset failed: {}", port.name, err);
                    }
                }
            } else {
                log::info!("Port {} DDI already active", port.name);
            }

            port.dump();
        }

        for dpll in dplls.iter() {
            dpll.dump();
        }

        for transcoder in transcoders.iter() {
            transcoder.dump();
        }

        for pipe in pipes.iter() {
            pipe.dump();
        }

        /*TODO: hotplug detect
        loop {
            //eprint!("\r");
            eprint!(" DE_HPD_INTERRUPT {:08X}", de_hpd_interrupt.read());
            eprint!(" DE_PORT_INTERRUPT {:08X}", de_port_interrupt.read());
            eprint!(" SDE_INTERRUPT {:08X}", sde_interrupt.read());
            eprint!(" SHOTPLUG_CTL_DDI {:08X}", shotplug_ctl_ddi.read());
            eprint!(" SHOTPLUG_CTL_TC {:08X}", shotplug_ctl_tc.read());
            eprint!(" TBT_HOTPLUG_CTL {:08X}", tbt_hotplug_ctl.read());
            eprint!(" TC_HOTPLUG_CTL {:08X}", tc_hotplug_ctl.read());
            eprintln!();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        */

        Ok(Self {
            kind,
            ddis,
            dplls,
            gttmm,
            gm,
            pipes,
            transcoders,
        })
    }
}