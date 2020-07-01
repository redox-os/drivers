use std::str;

use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};

use syscall::{
    error::{Error, Result},
    scheme::{self, Scheme},

    io_uring::{
        v1,
        IoUringRecvInfo,
        ProducerInstance, ProducerGenericReceiver, ProducerGenericSender,
    },

    EACCES, EBADF, EBADFD, EINVAL, ENOENT,

    SEEK_CUR, SEEK_END, SEEK_SET,

    O_CREAT, O_STAT, O_DIRECTORY, O_ACCMODE, O_RDONLY,
};

pub struct PcidScheme {
    next_handle: AtomicUsize,

    // TODO: Concurrent B-tree.
    io_uring_handles: RwLock<BTreeMap<usize, Handle>>,
}

struct Handle {
    ctx: scheme::Ctx,
    instance: RwLock<v1::ProducerInstance>,
}

const HANDLE_STAT: usize = 0;
const HANDLE_LIST: usize = 1;
const FIRST_HANDL: usize = 2;

impl PcidScheme {
    pub fn new() -> Self {
        Self {
            next_handle: AtomicUsize::new(FIRST_HANDL),
            io_uring_handles: RwLock::new(BTreeMap::new()),
        }
    }
}

// 
// The `pci:` scheme doesn't really do that much yet, although it may certainly be used to
// enumerate devices in the future. Currently, the only way to obtain a handle to it is through
// `SYS_ATTACH_IORING`, which always comes from a user process. The kernel may also attach a ring
// to `pcid`, but that's not meant to be used for anything.
//
// `pcid` allows `pci:/` to be listed and `fstat`ed, where it'll just return an empty list of
// files, for now.
//

impl Scheme for PcidScheme {
    fn open(&self, path: &[u8], flags: usize, uid: u32, gid: u32) -> Result<usize> {
        let path = str::from_utf8(path).or(Err(Error::new(ENOENT)))?.trim_start_matches('/');

        if !path.is_empty() {
            return Err(Error::new(ENOENT));
        }

        if flags & O_CREAT != 0 || flags & O_ACCMODE != O_RDONLY {
            return Err(Error::new(EACCES));
        }

        if flags & O_STAT != 0 {
            Ok(HANDLE_STAT)
        } else if flags & O_DIRECTORY != 0 {
            Ok(HANDLE_LIST)
        } else {
            Err(Error::new(ENOENT))
        }
    }
    fn seek(&self, id: usize, pos: isize, whence: usize) -> Result<isize> {
        if id != HANDLE_LIST && id != HANDLE_STAT {
            return Err(Error::new(EBADF));
        }
        if whence != SEEK_CUR && whence != SEEK_END && whence != SEEK_SET {
            return Err(Error::new(EINVAL));
        }
        Ok(0)
    }
    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        if id != HANDLE_LIST && id != HANDLE_STAT {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }

    fn recv_io_uring(&self, ctx: scheme::Ctx, info: &IoUringRecvInfo) -> Result<usize> {
        let fd = self.next_handle.fetch_add(1, Ordering::Relaxed);

        let instance = ProducerInstance::new_v1(info)?;
        
        let handle = Handle {
            ctx,
            instance: RwLock::new(instance),
        };

        if self.io_uring_handles.write().unwrap().insert(fd, handle).is_some() {
            println!("Already a handle at fd {}, returning EBADFD", fd);
            return Err(Error::new(EBADFD));
        }

        Ok(0)
    }
}
