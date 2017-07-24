use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::rc::Weak;

use syscall::error::{Error, Result, EBADF, EINVAL, EPIPE};
use syscall::flag::{F_GETFL, F_SETFL, O_ACCMODE};

use pty::Pty;
use resource::Resource;

/// Read side of a pipe
#[derive(Clone)]
pub struct PtyTermios {
    pty: Weak<RefCell<Pty>>,
    flags: usize,
}

impl PtyTermios {
    pub fn new(pty: Weak<RefCell<Pty>>, flags: usize) -> Self {
        PtyTermios {
            pty: pty,
            flags: flags,
        }
    }
}

impl Resource for PtyTermios {
    fn boxed_clone(&self) -> Box<Resource> {
        Box::new(self.clone())
    }

    fn pty(&self) -> Weak<RefCell<Pty>> {
        self.pty.clone()
    }

    fn flags(&self) -> usize {
        self.flags
    }

    fn path(&self, buf: &mut [u8]) -> Result<usize> {
        if let Some(pty_lock) = self.pty.upgrade() {
            pty_lock.borrow_mut().path(buf)
        } else {
            Err(Error::new(EPIPE))
        }
    }

    fn read(&self, buf: &mut [u8]) -> Result<usize> {
        if let Some(pty_lock) = self.pty.upgrade() {
            let pty = pty_lock.borrow();
            let termios: &[u8] = pty.termios.deref();

            let mut i = 0;
            while i < buf.len() && i < termios.len() {
                buf[i] = termios[i];
                i += 1;
            }
            Ok(i)
        } else {
            Ok(0)
        }
    }

    fn write(&self, buf: &[u8]) -> Result<usize> {
        if let Some(pty_lock) = self.pty.upgrade() {
            let mut pty = pty_lock.borrow_mut();
            let termios: &mut [u8] = pty.termios.deref_mut();

            let mut i = 0;
            while i < buf.len() && i < termios.len() {
                termios[i] = buf[i];
                i += 1;
            }
            Ok(i)
        } else {
            Err(Error::new(EPIPE))
        }
    }

    fn sync(&self) -> Result<usize> {
        Ok(0)
    }

    fn fcntl(&mut self, cmd: usize, arg: usize) -> Result<usize> {
        match cmd {
            F_GETFL => Ok(self.flags),
            F_SETFL => {
                self.flags = (self.flags & O_ACCMODE) | (arg & ! O_ACCMODE);
                Ok(0)
            },
            _ => Err(Error::new(EINVAL))
        }
    }

    fn fevent(&self) -> Result<()> {
        Err(Error::new(EBADF))
    }

    fn fevent_count(&self) -> Option<usize> {
        None
    }
}
