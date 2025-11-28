use common::{io::{Io, Mmio}, timeout::Timeout};
use pcid_interface::PciFunction;
use std::{mem, ptr};
use syscall::error::{Error, Result, EIO, ENODEV, ERANGE};

mod ddi;
use self::ddi::Ddi;

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
        log::info!("IOBAR {:X?}", iobar);

        let ddi = Ddi::new(kind)?;

        let mut pwr_well_ctl_aux = match kind {
            DeviceKind::TigerLake => {
                // IHD-OS-TGL-Vol 2c-12.21
                unsafe { gttmm.mmio(0x45444)? }
            },
        };
        log::info!("PWR_WELL_CTL_AUX {:08X}", pwr_well_ctl_aux.read());

        for port in ddi.ports.iter() {
            //let buf_ctl = unsafe { gttmm.mmio(port.buf_ctl())? };

            const AUX_CTL_BUSY: u32 = 1 << 31;
            const AUX_CTL_DONE: u32 = 1 << 30;
            const AUX_CTL_TIMEOUT_ERROR: u32 = 1 << 28;
            const AUX_CTL_RECEIVE_ERROR: u32 = 1 << 25;
            const AUX_CTL_SIZE_SHIFT: u32 = 20;
            const AUX_CTL_SIZE_MASK: u32 = 0b11111 << 20;
            let aux_ctl = unsafe { gttmm.mmio(port.aux_ctl())? };

            enum I2CData<'a> {
                Read(&'a mut [u8]),
                Write(&'a [u8]),
            }

            let mut i2c_tx = |mot: bool, addr: u8, mut data: I2CData| -> Result<()> {
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
                // Start transaction
                v |= AUX_CTL_BUSY;
                aux_ctl.write(v);

                // Wait while busy
                let timeout = Timeout::from_secs(1);
                while aux_ctl.readf(AUX_CTL_BUSY) {
                    timeout.run().map_err(|()| {
                        log::error!("AUX I2C transaction wait timeout");
                        Error::new(EIO)
                    })?;
                }

                // Read result
                v = aux_ctl.read();
                if (v & AUX_CTL_TIMEOUT_ERROR) != 0 {
                    log::error!("AUX I2C transaction timeout error");
                    return Err(Error::new(EIO));
                } 
                if (v & AUX_CTL_RECEIVE_ERROR) != 0 {
                    log::error!("AUX I2C transaction receive error");
                    return Err(Error::new(EIO));
                } 
                if (v & AUX_CTL_DONE) == 0 {
                    log::error!("AUX I2C transaction done not set");
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
                    log::error!("AUX I2C unexpected response {:02X}", response);
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

            let mut read_edid = || -> Result<[u8; 128]> {
                // Enable AUX power
                //TODO: disable power with guard?
                let timeout = Timeout::from_micros(1500);
                pwr_well_ctl_aux.writef(port.pwr_well_ctl_aux_request(), true);
                while !pwr_well_ctl_aux.readf(port.pwr_well_ctl_aux_state()) {
                    timeout.run().map_err(|()| {
                        log::error!("timeout while requesting port {} aux power", port.name);
                        Error::new(EIO)
                    })?;
                }

                // Check if device responds
                i2c_tx(true, 0x50, I2CData::Write(&[]))?;
                // Write index
                i2c_tx(true, 0x50, I2CData::Write(&[0]))?;
                // Read EDID
                //TODO: Could EDID be read in multiple byte transactions?
                let mut edid = [0; 128];
                for chunk in edid.chunks_mut(1) {
                    i2c_tx(true, 0x50, I2CData::Read(chunk))?;
                }
                // Finish transaction
                i2c_tx(false, 0x50, I2CData::Read(&mut []))?;

                Ok(edid)
            };

            match read_edid() {
                Ok(edid) => {
                    log::info!("Port {} EDID: {:x?}", port.name, edid);
                    let (width, height) = (
                        (edid[0x38] as u32) | (((edid[0x3A] as u32) & 0xF0) << 4),
                        (edid[0x3B] as u32) | (((edid[0x3D] as u32) & 0xF0) << 4),
                    );
                    log::info!("Port {} best resolution:: {}x{}", port.name, width, height);
                },
                Err(_) => ()
            }
        }

        log::info!("PWR_WELL_CTL_AUX {:08X}", pwr_well_ctl_aux.read());

        Ok(Self {
            kind,
            ddi,
            gttmm,
            gm,
        })
    }
}