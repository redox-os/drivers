use syscall::error::Result;

use super::DeviceKind;

#[derive(Debug)]
pub struct DdiPort {
    pub name: &'static str,
    pub index: usize,
}

//TODO: verify offsets and count using DeviceKind?
impl DdiPort {
    pub fn new(name: &'static str, index: usize) -> Self {
        Self { name, index }
    }

    pub fn addr(&self) -> usize {
        0x64000 + (self.index * 0x100)
    }

    pub fn buf_ctl(&self) -> usize {
        self.addr()
    }

    pub fn aux_ctl(&self) -> usize {
        self.addr() + 0x10
    }

    pub fn gmbus_pin_pair(&self) -> Option<u8> {
        match self.index {
            // DDI pins
            0 => Some(1),
            1 => Some(2),
            2 => Some(3),
            // Type C pins
            3 => Some(9),
            4 => Some(10),
            5 => Some(11),
            6 => Some(12),
            7 => Some(13),
            8 => Some(14),
            _ => None
        }
    }

    pub fn port_cl_dw10(&self) -> Option<usize> {
        match self.index {
            0 => Some(0x162028),
            1 => Some(0x6C028),
            2 => Some(0x160028),
            _ => None,
        }
    }

    pub fn port_comp_dw0(&self) -> Option<usize> {
        match self.index {
            0 => Some(0x162100),
            1 => Some(0x6C100),
            2 => Some(0x160100),
            _ => None,
        }
    }

    pub fn pwr_well_ctl_aux_state(&self) -> u32 {
        1 << (self.index * 2)
    }

    pub fn pwr_well_ctl_aux_request(&self) -> u32 {
        2 << (self.index * 2)
    }

    pub fn pwr_well_ctl_ddi_state(&self) -> u32 {
        1 << (self.index * 2)
    }

    pub fn pwr_well_ctl_ddi_request(&self) -> u32 {
        2 << (self.index * 2)
    }

    pub fn aux_datas(&self) -> [usize; 5] {
        let addr = self.addr();
        [
            addr + 0x14,
            addr + 0x18,
            addr + 0x1C,
            addr + 0x20,
            addr + 0x24,
        ]
    }
}

#[derive(Debug)]
pub struct Ddi {
    pub ports: Vec<DdiPort>
}

impl Ddi {
    pub fn new(kind: DeviceKind) -> Result<Self> {
        match kind {
            DeviceKind::TigerLake => {
                // IHD-OS-TGL-Vol 2c-12.21
                Ok(Self {
                    ports: vec![
                        DdiPort::new("A", 0),
                        DdiPort::new("B", 1),
                        DdiPort::new("C", 2),
                        DdiPort::new("USBC1", 3),
                        DdiPort::new("USBC2", 4),
                        DdiPort::new("USBC3", 5),
                        DdiPort::new("USBC4", 6),
                        DdiPort::new("USBC5", 7),
                        DdiPort::new("USBC6", 8),
                    ]
                })
            }
        }
    }
}