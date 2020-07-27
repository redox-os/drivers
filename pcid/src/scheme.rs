use std::str;
use std::sync::{Arc, RwLock, Weak};

use syscall::error::{Error, Result};
use syscall::data::Stat;
use syscall::error::{EACCES, EBADF, EINVAL, ENOENT, ENOSYS};
use syscall::flag::{
    MODE_DIR,
    O_CREAT, O_STAT, O_DIRECTORY, O_ACCMODE, O_RDONLY,
    SEEK_CUR, SEEK_END, SEEK_SET,
};
use syscall::io_uring::IoUringRecvInfo;
use syscall::io_uring::v1::{CqEntry64, IoUringSqeFlags, Priority, StandardOpcode, SqEntry64};
use syscall::scheme::{self, Scheme};

use redox_iou::executor::SpawnHandle;
use redox_iou::instance::ProducerInstance;
use redox_iou::reactor::SecondaryRingId;
use redox_iou::{memory::pool as redox_iou_pool, reactor};

use futures::StreamExt;

use crate::{DeviceTree, Func, ResultExt};
use crate::driver_interface::{PciAddress64, PcidOpcode};

pub struct PcidScheme {
    spawn_handle: SpawnHandle,
    reactor_handle: reactor::Handle,
    self_weak: Option<Weak<Self>>,
    tree: Arc<DeviceTree>,
}

const HANDLE_STAT: usize = 0;
const HANDLE_LIST: usize = 1;

impl PcidScheme {
    pub fn new(spawn_handle: SpawnHandle, reactor_handle: reactor::Handle, tree: Arc<DeviceTree>) -> Arc<Self> {
        let mut self_arc = Arc::new(Self {
            spawn_handle,
            reactor_handle,
            self_weak: None,
            tree,
        });
        let self_weak = Arc::downgrade(&self_arc);
        Arc::get_mut(&mut self_arc).unwrap().self_weak = Some(self_weak);
        self_arc
    }
    fn self_weak(&self) -> &Weak<Self> {
        self.self_weak.as_ref().expect("expected PcidScheme to actually contain a self-ref after init")
    }
    fn self_arc(&self) -> Arc<Self> {
        self.self_weak().upgrade().expect("how would one get a ref to PciScheme if the Arc is dead?")
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
        log::trace!("PCI SCHEME OPEN PATH=`{}` FLAGS={:#0x} uid={} gid={}", path, flags, uid, gid);

        if !path.is_empty() {
            return Err(Error::new(ENOENT));
        }

        if (flags & O_CREAT != 0 || flags & O_ACCMODE != O_RDONLY) && flags & O_STAT == 0 {
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
        log::trace!("PCI SCHEME SEEK FD={} POS={} whence={}", id, pos, whence);

        if id != HANDLE_LIST && id != HANDLE_STAT {
            return Err(Error::new(EBADF));
        }
        if whence != SEEK_CUR && whence != SEEK_END && whence != SEEK_SET {
            return Err(Error::new(EINVAL));
        }
        Ok(0)
    }
    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        log::trace!("PCI SCHEME READ FD={} BUF=<AT {:p} LEN {}>", id, buf.as_mut_ptr(), buf.len());

        if id != HANDLE_LIST && id != HANDLE_STAT {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
    fn fstat(&self, id: usize, stat: &mut Stat) -> Result<usize> {
        log::trace!("PCI SCHEME FSTAT FD={}, STAT=<`Stat` AT {:p}>", id, stat as *mut Stat);

        if id == HANDLE_STAT || id == HANDLE_LIST {
            *stat = Stat {
                st_dev: 0,
                st_blksize: 4096,
                st_blocks: 0,
                st_size: 0,
                st_ino: 0,

                st_mode: MODE_DIR | 0o555,
                st_nlink: 1,

                st_uid: 0,
                st_gid: 0,

                st_atime: 0,      // TODO
                st_atime_nsec: 0, // TODO
                st_ctime: 0,      // TODO
                st_ctime_nsec: 0, // TODO
                st_mtime: 0,      // TODO
                st_mtime_nsec: 0, // TODO
            };
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn recv_io_uring(&self, ctx: scheme::Ctx, info: &IoUringRecvInfo) -> Result<usize> {
        log::debug!(
            "PCI SCHEME RECV_IOURING CTX=<PID={pid} UID={uid} GID={gid}> VERSION={major}.{minor}.{patch}",
            pid=ctx.pid, uid=ctx.uid, gid=ctx.gid, major=info.version.major,
            minor=info.version.minor, patch=info.version.patch);

        let instance = ProducerInstance::new(info).and_log_err_as_warn("failed to create producer instance")?;
        
        let reactor_handle = self.reactor_handle.clone();
        let ring = reactor_handle.reactor().add_producer_instance(instance, Priority::default()).and_log_err_as_warn("failed to register producer instance to reactor")?;
        let mut stream = reactor_handle.producer_sqes(ring, 64);

        let this = self.self_arc();

        self.spawn_handle.spawn(async move {
            log::info!("Spawning works");
            let mut pool = None;

            while let Some(sqe) = stream.next().await {
                log::info!("PCI SCHEME RECV SQE {:?}", sqe);
                let sqe = match sqe {
                    Ok(sqe) => sqe,
                    Err(error) => {
                        log::warn!("Failed to receive SQE: {}, dropping that ring.", error);
                        return;
                    }
                };

                /*let cqe = */if let Some(standard_opcode) = StandardOpcode::from_raw(sqe.opcode) {
                    let cqe_res = this.handle_standard_opcode(standard_opcode, &sqe).await;
                    let _ = Self::or_error(&sqe, cqe_res, 0);
                } else if let Some(pcid_opcode) = PcidOpcode::from_raw(sqe.opcode) {
                    let cqe_res = this.handle_pcid_opcode(&ctx, &reactor_handle, ring, &mut pool, pcid_opcode, &sqe).await;
                    let _ = Self::or_error(&sqe, cqe_res, 0);
                };/* else*/ let cqe = {
                    CqEntry64 {
                        user_data: sqe.user_data,
                        flags: 0, // TODO
                        status: Error::mux64(Err(Error::new(ENOSYS))),
                        extra: 0,
                    }
                };

                match reactor_handle.send_producer_cqe(ring, cqe) {
                    Ok(()) => (),
                    Err(error) => {
                        log::warn!("Failed to send CQE: {}, dropping that ring.", error);
                        return;
                    }
                }
            }
        });

        Ok(0)
    }
}
impl PcidScheme {
    // TODO: Make it possible to get an Arc of the driver, to avoid lifetime errors with the
    // executor.
    pub async fn handle_standard_opcode(&self, opcode: StandardOpcode, sqe: &SqEntry64) -> Result<CqEntry64> {
        log::warn!("TODO: handle standard opcode {:?}, sqe {:?}", opcode, sqe);
        Ok(CqEntry64::default())
    }
    pub async fn handle_pcid_opcode(&self, ctx: &scheme::Ctx, reactor: &reactor::Handle, ring: SecondaryRingId, pool: &mut Option<redox_iou::memory::BufferPool>, opcode: PcidOpcode, sqe: &SqEntry64) -> Result<CqEntry64> {
        let pool = match pool {
            Some(p) => p,
            None => {
                log::info!("Creating producer pool");
                let new_pool = reactor.create_producer_buffer_pool(ring, Priority::default()).await?;
                *pool = Some(new_pool);
                pool.as_mut().unwrap()
            },
        };
        log::warn!("Buffer pool initialized; TODO");

        fn check_if_version(sqe: &SqEntry64) -> Result<()> {
            let pcid_if_version = sqe.syscall_flags;
            if pcid_if_version != 1 {
                return Err(Error::new(ENOSYS));
            }
            Ok(())
        }
        fn parse_pci_addr(sqe: &SqEntry64) -> PciAddress64 {
            *plain::from_bytes(&sqe.fd.to_ne_bytes()).expect("the fd field of SqEntry64 has insufficient alignment")
        }
        fn validate_pci_addr(ctx: &scheme::Ctx, _addr: PciAddress64) -> Result<()> {
            // TODO
            if ctx.uid == 0 {
                Ok(())
            } else {
                Err(Error::new(EACCES))
            }
        }
        fn find_device(tree: &DeviceTree, addr: PciAddress64) -> Result<Arc<RwLock<Func>>> {
            Ok(Arc::clone(tree.get(&addr.base).ok_or(Error::new(ENOENT))?))
        }

        match opcode {
            PcidOpcode::FetchConfig => {
                check_if_version(sqe)?;
                let addr = parse_pci_addr(sqe);
                validate_pci_addr(ctx, addr)?;
                let dev = find_device();
            },
            PcidOpcode::FetchAllCapabilities => {
                check_if_version(sqe)?;
                let addr = parse_pci_addr(sqe);
                validate_pci_addr(ctx, addr)?;
            },
            PcidOpcode::GetCapability => {
                check_if_version(sqe)?;
                let addr = parse_pci_addr(sqe);
                validate_pci_addr(ctx, addr)?;
            },
            PcidOpcode::SetCapability => {
                check_if_version(sqe)?;
                let addr = parse_pci_addr(sqe);
                validate_pci_addr(ctx, addr)?;
            },
        }

        Ok(CqEntry64::default())
    }
    fn or_error(sqe: &SqEntry64, result: Result<CqEntry64>, extra: u64) -> CqEntry64 {
        result.unwrap_or_else(|err| CqEntry64 {
            user_data: sqe.user_data,
            flags: 0, // TODO
            extra,
            status: Error::mux64(Err(err)),
        })
    }
}
