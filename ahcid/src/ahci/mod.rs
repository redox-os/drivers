use syscall::io::Io;
use syscall::error::Result;

use self::disk_ata::DiskATA;
use self::disk_atapi::DiskATAPI;
use self::hba::{HbaMem, HbaPortType};

pub mod disk_ata;
pub mod disk_atapi;
pub mod fis;
pub mod hba;

pub trait Disk {
    fn id(&self) -> usize;
    fn size(&mut self) -> u64;
    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize>;
    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<usize>;
}

pub fn disks(base: usize, name: &str) -> Vec<Box<Disk>> {
    unsafe { &mut *(base as *mut HbaMem) }.init();
    let pi = unsafe { &mut *(base as *mut HbaMem) }.pi.read();
    let ret: Vec<Box<Disk>> = (0..32)
          .filter(|&i| pi & 1 << i as i32 == 1 << i as i32)
          .filter_map(|i| {
              let port = &mut unsafe { &mut *(base as *mut HbaMem) }.ports[i];
              let port_type = port.probe();
              print!("{}", format!("{}-{}: {:?}\n", name, i, port_type));

              let disk: Option<Box<Disk>> = match port_type {
                  HbaPortType::SATA => {
                      match DiskATA::new(i, port) {
                          Ok(disk) => Some(Box::new(disk)),
                          Err(err) => {
                              print!("{}", format!("{}: {}\n", i, err));
                              None
                          }
                      }
                  }
                  HbaPortType::SATAPI => {
                      match DiskATAPI::new(i, port) {
                          Ok(disk) => Some(Box::new(disk)),
                          Err(err) => {
                              print!("{}", format!("{}: {}\n", i, err));
                              None
                          }
                      }
                  }
                  _ => None,
              };

              disk
          })
          .collect();

    ret
}
