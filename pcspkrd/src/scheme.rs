use syscall::data::Stat;
use syscall::{
    Error, Result, SchemeMut, EBUSY, EINVAL, EPERM, MODE_CHR, O_ACCMODE, O_STAT, O_WRONLY,
};

use crate::pcspkr::Pcspkr;

pub struct PcspkrScheme {
    pub pcspkr: Pcspkr,
    pub handle: Option<usize>,
    pub next_id: usize,
}

impl SchemeMut for PcspkrScheme {
    fn open(&mut self, _path: &str, flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        if (flags & O_ACCMODE == 0) && (flags & O_STAT == O_STAT) {
            Ok(0)
        } else if flags & O_ACCMODE == O_WRONLY {
            if self.handle.is_none() {
                self.next_id += 1;
                self.handle = Some(self.next_id);
                Ok(self.next_id)
            } else {
                Err(Error::new(EBUSY))
            }
        } else {
            Err(Error::new(EINVAL))
        }
    }

    fn dup(&mut self, _id: usize, _buf: &[u8]) -> Result<usize> {
        Err(Error::new(EPERM))
    }

    fn read(&mut self, _id: usize, _buf: &mut [u8]) -> Result<usize> {
        Err(Error::new(EPERM))
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        if self.handle != Some(id) {
            return Err(Error::new(EINVAL));
        }

        if buf.len() != 2 {
            return Err(Error::new(EINVAL));
        }

        let frequency = buf[0] as usize + ((buf[1] as usize) << 8);

        if frequency == 0 {
            self.pcspkr.set_gate(false);
        } else {
            self.pcspkr.set_frequency(frequency);
            self.pcspkr.set_gate(true);
        }

        Ok(buf.len())
    }

    fn fpath(&mut self, _id: usize, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;
        let scheme_path = b"pcspkr";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(i)
    }

    fn fstat(&mut self, _id: usize, stat: &mut Stat) -> Result<usize> {
        *stat = Stat {
            st_mode: MODE_CHR | 0o222,
            ..Default::default()
        };

        Ok(0)
    }

    fn fcntl(&mut self, _id: usize, _cmd: usize, _arg: usize) -> Result<usize> {
        Ok(0)
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        if self.handle == Some(id) {
            self.pcspkr.set_gate(false);
            self.handle = None;
        }

        Ok(0)
    }
}
