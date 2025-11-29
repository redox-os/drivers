use common::{io::{Io, Mmio}, timeout::Timeout};
use pcid_interface::PciFunction;
use std::{mem, ptr};
use syscall::error::{Error, Result, EIO, ENODEV, ERANGE};

mod ddi;
use self::ddi::Ddi;
mod transcoder;
use self::transcoder::Transcoder;

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

    unsafe fn mmio(&self, offset: usize) -> Result<&'static mut Mmio<u32>> {
        if offset + mem::size_of::<u32>() > self.size {
            return Err(Error::new(ERANGE));
        }
        let addr = self.virt + offset;
        Ok(unsafe { &mut *(addr as *mut Mmio<u32>) })
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
    ddi: Ddi,
    gttmm: MmioRegion,
    gm: MmioRegion,
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
            MmioRegion::new(phys, size)?
        };
        let gm = {
            let (phys, size) = func.bars[2].expect_mem();
            MmioRegion::new(phys, size)?
        };
        let iobar = func.bars[4].expect_port();
        log::debug!("IOBAR {:X?}", iobar);

        let ddi = Ddi::new(kind)?;

        let de_hpd_interrupt;
        let de_port_interrupt;
        let mut gmbus;
        let mut pwr_well_ctl_aux;
        let mut pwr_well_ctl_ddi;
        let sde_interrupt;
        let shotplug_ctl_ddi;
        let shotplug_ctl_tc;
        let tbt_hotplug_ctl;
        let tc_hotplug_ctl;
        let transcoders;
        match kind {
            DeviceKind::TigerLake => {
                // IHD-OS-TGL-Vol 2c-12.21
                let dc_state_en = unsafe { gttmm.mmio(0x45504)? };
                log::debug!("dc_state_en {:08X}", dc_state_en.read());

                de_hpd_interrupt = unsafe { gttmm.mmio(0x44470)? };
                log::debug!("de_hpd_interrupt {:08X}", de_hpd_interrupt.read());

                de_port_interrupt = unsafe { gttmm.mmio(0x44440)? };
                log::debug!("de_port_interrupt {:08X}", de_port_interrupt.read());

                let dpclka_cfgcr0 = unsafe { gttmm.mmio(0x164280)? };
                log::info!("dpclka_cfgcr0 {:08X}", dpclka_cfgcr0.read());

                let dpll0_cfgcr0 = unsafe { gttmm.mmio(0x164284)? };
                log::info!("dpll0_cfgcr0 {:08X}", dpll0_cfgcr0.read());

                let dpll0_cfgcr1 = unsafe { gttmm.mmio(0x164288)? };
                log::info!("dpll0_cfgcr1 {:08X}", dpll0_cfgcr1.read());

                let dpll0_enable = unsafe { gttmm.mmio(0x46010)? };
                log::info!("dpll0_enable {:08X}", dpll0_enable.read());

                let dpll1_enable = unsafe { gttmm.mmio(0x46014)? };
                log::info!("dpll1_enable {:08X}", dpll1_enable.read());

                let dpll4_enable = unsafe { gttmm.mmio(0x46018)? };
                log::info!("dpll4_enable {:08X}", dpll4_enable.read());

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

                let trans_clk_sel_a = unsafe { gttmm.mmio(0x46140)? };
                log::info!("trans_clk_sel_a {:08X}", trans_clk_sel_a.read());

                let trans_clk_sel_b = unsafe { gttmm.mmio(0x46144)? };
                log::info!("trans_clk_sel_b {:08X}", trans_clk_sel_b.read());

                let trans_clk_sel_c = unsafe { gttmm.mmio(0x46148)? };
                log::info!("trans_clk_sel_c {:08X}", trans_clk_sel_c.read());

                let trans_clk_sel_d = unsafe { gttmm.mmio(0x4614C)? };
                log::info!("trans_clk_sel_d {:08X}", trans_clk_sel_d.read());

                transcoders = Transcoder::tigerlake(&gttmm)?;
            },
        };

        for port in ddi.ports.iter() {
            //TODO: init port if needed
            if let Some(offset) = port.port_comp_dw0() {
                let port_comp_dw0 = unsafe { gttmm.mmio(offset)? };
                log::debug!("PORT_COMP_DW0_{}: {:08X}", port.name, port_comp_dw0.read());
            }

            const AUX_CTL_BUSY: u32 = 1 << 31;
            const AUX_CTL_DONE: u32 = 1 << 30;
            const AUX_CTL_TIMEOUT_ERROR: u32 = 1 << 28;
            const AUX_CTL_TIMEOUT_SHIFT: u32 = 26;
            const AUX_CTL_TIMEOUT_MASK: u32 = 0b11 << AUX_CTL_TIMEOUT_SHIFT;
            const AUX_CTL_TIMEOUT_4000US: u32 = 0b11 << AUX_CTL_TIMEOUT_SHIFT;
            const AUX_CTL_RECEIVE_ERROR: u32 = 1 << 25;
            const AUX_CTL_SIZE_SHIFT: u32 = 20;
            const AUX_CTL_SIZE_MASK: u32 = 0b11111 << 20;
            const AUX_CTL_IO_SELECT: u32 = 1 << 11;
            let aux_ctl = unsafe { gttmm.mmio(port.aux_ctl())? };

            enum I2CData<'a> {
                Read(&'a mut [u8]),
                Write(&'a [u8]),
            }

            let mut aux_i2c_tx = |mot: bool, addr: u8, mut data: I2CData| -> Result<()> {
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
                    let mut aux_data = unsafe { gttmm.mmio(port.aux_datas()[i])? };
                    let mut bytes = [0; 4];
                    bytes[..chunk.len()].copy_from_slice(&chunk);
                    aux_data.write(u32::from_be_bytes(bytes));
                }

                let mut v = aux_ctl.read();
                // Set length
                v &= !AUX_CTL_SIZE_MASK;
                v |= (aux_data_i as u32) << AUX_CTL_SIZE_SHIFT;
                // Set timeout
                v &= !AUX_CTL_TIMEOUT_MASK;
                v |= AUX_CTL_TIMEOUT_4000US;
                // Set I/O select to legacy (cleared)
                //TODO: TBT support?
                v &= !AUX_CTL_IO_SELECT;
                // Start transaction
                v |= AUX_CTL_BUSY;
                aux_ctl.write(v);

                // Wait while busy
                let timeout = Timeout::from_secs(1);
                while aux_ctl.readf(AUX_CTL_BUSY) {
                    timeout.run().map_err(|()| {
                        log::debug!("AUX I2C transaction wait timeout");
                        Error::new(EIO)
                    })?;
                }

                // Read result
                v = aux_ctl.read();
                if (v & AUX_CTL_TIMEOUT_ERROR) != 0 {
                    log::debug!("AUX I2C transaction timeout error");
                    return Err(Error::new(EIO));
                } 
                if (v & AUX_CTL_RECEIVE_ERROR) != 0 {
                    log::debug!("AUX I2C transaction receive error");
                    return Err(Error::new(EIO));
                } 
                if (v & AUX_CTL_DONE) == 0 {
                    log::debug!("AUX I2C transaction done not set");
                    return Err(Error::new(EIO));
                }

                // Read data from registers (big endian, dword access only)
                for (i, chunk) in aux_datas.chunks_mut(4).enumerate() {
                    let mut aux_data = unsafe { gttmm.mmio(port.aux_datas()[i])? };
                    let bytes = aux_data.read().to_be_bytes();
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

            let mut aux_read_edid = || -> Result<[u8; 128]> {
                //TODO: BLOCK TCCOLD?

                let _pwr_guard = CallbackGuard::new(
                    &mut pwr_well_ctl_aux,
                    |pwr_well_ctl_aux| {
                        // Enable aux power
                        pwr_well_ctl_aux.writef(port.pwr_well_ctl_aux_request(), true);
                        let timeout = Timeout::from_micros(1500);
                        while !pwr_well_ctl_aux.readf(port.pwr_well_ctl_aux_state()) {
                            timeout.run().map_err(|()| {
                                log::debug!("timeout while requesting port {} aux power", port.name);
                                Error::new(EIO)
                            })?;
                        }
                        Ok(())
                    },
                    |pwr_well_ctl_aux| {
                        // Disable aux power
                        pwr_well_ctl_aux.writef(port.pwr_well_ctl_aux_request(), false);
                    }
                )?;

                // Check if device responds
                aux_i2c_tx(true, 0x50, I2CData::Write(&[]))?;
                // Write index
                aux_i2c_tx(true, 0x50, I2CData::Write(&[0]))?;
                // Read EDID
                //TODO: Could EDID be read in multiple byte transactions?
                let mut edid = [0; 128];
                for chunk in edid.chunks_mut(1) {
                    aux_i2c_tx(true, 0x50, I2CData::Read(chunk))?;
                }
                // Finish transaction
                aux_i2c_tx(false, 0x50, I2CData::Read(&mut []))?;

                Ok(edid)
            };

            let mut gmbus_i2c_tx = |addr7: u8, index: u8, mut data: I2CData| -> Result<()> {
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

            let mut gmbus_read_edid = || -> Result<[u8; 128]> {
                let mut edid = [0; 128];
                gmbus_i2c_tx(0x50, 0x00, I2CData::Read(&mut edid))?;
                Ok(edid)
            };

            let (source, edid) = match aux_read_edid() {
                Ok(edid) => ("AUX", edid),
                Err(err) => {
                    log::debug!("Port {} failed to read EDID from AUX: {}", port.name, err);
                    match gmbus_read_edid() {
                        Ok(edid) => ("GMBUS", edid),
                        Err(err) => {
                            log::debug!("Port {} failed to read EDID from GMBUS: {}", port.name, err);
                            continue;
                        }
                    }
                }
            };

            log::debug!("Port {} EDID from {}: {:x?}", port.name, source, edid);
            let (width, height) = (
                (edid[0x38] as u32) | (((edid[0x3A] as u32) & 0xF0) << 4),
                (edid[0x3B] as u32) | (((edid[0x3D] as u32) & 0xF0) << 4),
            );
            log::info!("Port {} best resolution using EDID from {}: {}x{}", port.name, source, width, height);

            const DDI_BUF_CTL_EANBLE: u32 = 1 << 31;
            const DDI_BUF_CTL_IDLE: u32 = 1 << 7;

            let mut modeset_hdmi = |buf_ctl: &mut Mmio<u32>| -> Result<()> {
                // IHD-OS-TGL-Vol 12-1.22-Rev2.0 "Sequences for HDMI and DVI"

                // Power wells should already be enabled

                //TODO: Type-C needs aux power enabled and max lanes set
                
                // Enable port PLL without SSC
                //TODO: assuming a DPLL is already set up for this DDI!
                //TODO: Check DPCLKA_CFGCR0 for mapping and DPLL_ENABLE for status

                // Enable IO power
                let _pwr_guard = CallbackGuard::new(
                    &mut pwr_well_ctl_ddi,
                    |pwr_well_ctl_ddi| {
                        // Enable IO power
                        pwr_well_ctl_ddi.writef(port.pwr_well_ctl_ddi_request(), true);
                        let timeout = Timeout::from_micros(30);
                        while !pwr_well_ctl_ddi.readf(port.pwr_well_ctl_ddi_state()) {
                            timeout.run().map_err(|()| {
                                log::debug!("timeout while requesting port {} IO power", port.name);
                                Error::new(EIO)
                            })?;
                        }
                        Ok(())
                    },
                    |pwr_well_ctl_ddi| {
                        // Disable IO power
                        pwr_well_ctl_ddi.writef(port.pwr_well_ctl_ddi_request(), false);
                    }
                )?;

                //TODO: Type-C DP_MODE

                // Enable planes, pipe, and transcoder
                {
                    // Configure transcoder clock select

                    // Configure and enable planes

                    //TODO: VGA and panel fitter steps

                    // Configure transcoder timings and other pipe and transcoder settings

                    // Configure and enable TRANS_DDI_FUNC_CTL

                    // Configure and enable TRANS_CONF
                }

                // Enable port
                {
                    //TODO: Configure voltage swing and related IO settings

                    // Configure PORT_CL_DW10 static power down to power up all lanes
                    //TODO: only power up required lanes
                    if let Some(offset) = port.port_cl_dw10() {
                        let mut port_cl_dw10 = unsafe { gttmm.mmio(offset)? };
                        log::info!("port_cl_dw10 {:08X}", port_cl_dw10.read());
                        port_cl_dw10.writef(0b1111 << 4, false);
                    }

                    // Configure and enable DDI_BUF_CTL
                    //TODO: more DDI_BUF_CTL bits?
                    buf_ctl.writef(DDI_BUF_CTL_EANBLE, true);

                    // Wait for DDI_BUF_CTL IDLE = 0, timeout after 500 us
                    let timeout = Timeout::from_micros(500);
                    while buf_ctl.readf(DDI_BUF_CTL_IDLE) {
                        timeout.run().map_err(|()| {
                            log::warn!("timeout while waiting for port {} DDI active", port.name);
                            Error::new(EIO)
                        })?;
                    }
                }

                // Keep IO power on if finished
                mem::forget(_pwr_guard);

                Ok(())
            };

            let buf_ctl = unsafe { gttmm.mmio(port.buf_ctl())? };
            if buf_ctl.readf(DDI_BUF_CTL_IDLE) {
                log::info!("Port {} DDI idle, will attempt mode setting", port.name);
                //TODO: DisplayPort modeset
                match modeset_hdmi(buf_ctl) {
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
        }

        for transcoder in transcoders.iter() {
            transcoder.dump();
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
            ddi,
            gttmm,
            gm,
            transcoders,
        })
    }
}