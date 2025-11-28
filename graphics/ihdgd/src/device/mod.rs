use common::{io::{Io, Mmio}, timeout::Timeout};
use pcid_interface::PciFunction;
use std::{mem, ptr};
use syscall::error::{Error, Result, EIO, ENODEV, ERANGE};

mod ddi;
use self::ddi::Ddi;

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

#[derive(Debug)]
pub struct Device {
    kind: DeviceKind,
    ddi: Ddi,
    gttmm: MmioRegion,
    gm: MmioRegion,
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

        let mut gmbus;
        let mut pwr_well_ctl_aux;
        let mut pwr_well_ctl_ddi;
        match kind {
            DeviceKind::TigerLake => {
                // IHD-OS-TGL-Vol 2c-12.21
                let dc_state_en = unsafe { gttmm.mmio(0x45504)? };
                log::debug!("DC_STATE_EN {:08X}", dc_state_en.read());

                let fuse_status = unsafe { gttmm.mmio(0x42000)? };
                log::debug!("FUSE_STATUS {:08X}", fuse_status.read());

                gmbus = unsafe { [
                    gttmm.mmio(0xC5100)?,
                    gttmm.mmio(0xC5104)?,
                    gttmm.mmio(0xC5108)?,
                    gttmm.mmio(0xC510C)?,
                    gttmm.mmio(0xC5110)?,
                    gttmm.mmio(0xC5120)?,
                ] };

                let pwr_well_ctl = unsafe { gttmm.mmio(0x45404)? };
                log::debug!("PWR_WELL_CTL {:08X}", pwr_well_ctl.read());

                pwr_well_ctl_aux = unsafe { gttmm.mmio(0x45444)? };
                log::debug!("PWR_WELL_CTL_AUX {:08X}", pwr_well_ctl_aux.read());

                pwr_well_ctl_ddi = unsafe { gttmm.mmio(0x45454)? };
                log::debug!("PWR_WELL_CTL_DDI {:08X}", pwr_well_ctl_ddi.read());
            },
        };

        for port in ddi.ports.iter() {
            //TODO: init port if needed
            if let Some(offset) = port.port_comp_dw0() {
                let port_comp_dw0 = unsafe { gttmm.mmio(offset)? };
                log::debug!("PORT_COMP_DW0_{}: {:08X}", port.name, port_comp_dw0.read());
            }

            //let buf_ctl = unsafe { gttmm.mmio(port.buf_ctl())? };

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
        }

        Ok(Self {
            kind,
            ddi,
            gttmm,
            gm,
        })
    }
}