use inputd::ProducerHandle;
use redox_scheme::Scheme;
use std::str;
use syscall::data::Stat;
use syscall::{Error, Result, EACCES, EINVAL, MODE_CHR};

use crate::bga::Bga;

pub struct BgaScheme {
    pub bga: Bga,
    pub display: Option<ProducerHandle>,
}

impl BgaScheme {
    pub fn update_size(&mut self) {
        if let Some(ref mut display) = self.display {
            let _ = display.write_event(
                orbclient::ResizeEvent {
                    width: self.bga.width() as u32,
                    height: self.bga.height() as u32,
                }
                .to_event(),
            );
        }
    }
}

impl Scheme for BgaScheme {
    fn open(&mut self, _path: &str, _flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid == 0 {
            Ok(0)
        } else {
            Err(Error::new(EACCES))
        }
    }

    fn read(
        &mut self,
        _id: usize,
        buf: &mut [u8],
        _offset: u64,
        _fcntl_flags: u32,
    ) -> Result<usize> {
        let mut i = 0;
        let data = format!("{},{}\n", self.bga.width(), self.bga.height()).into_bytes();
        while i < buf.len() && i < data.len() {
            buf[i] = data[i];
            i += 1;
        }
        Ok(i)
    }

    fn write(&mut self, _id: usize, buf: &[u8], _offset: u64, _fcntl_flags: u32) -> Result<usize> {
        let string = str::from_utf8(buf).or(Err(Error::new(EINVAL)))?;
        let string = string.trim();

        let mut parts = string.split(',');

        let width = if let Some(part) = parts.next() {
            part.parse::<u16>().or(Err(Error::new(EINVAL)))?
        } else {
            self.bga.width()
        };

        let height = if let Some(part) = parts.next() {
            part.parse::<u16>().or(Err(Error::new(EINVAL)))?
        } else {
            self.bga.height()
        };

        self.bga.set_size(width, height);

        self.update_size();

        Ok(buf.len())
    }

    fn fpath(&mut self, _file: usize, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;
        let scheme_path = b"bga";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(i)
    }

    fn fstat(&mut self, _id: usize, stat: &mut Stat) -> Result<usize> {
        *stat = Stat {
            st_mode: MODE_CHR | 0o666,
            ..Default::default()
        };

        Ok(0)
    }

    fn fcntl(&mut self, _id: usize, _cmd: usize, _arg: usize) -> Result<usize> {
        Ok(0)
    }
}

impl BgaScheme {
    pub fn on_close(&mut self, _id: usize) {}
}
