use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::{cmp, io, mem, str};

use syscall::data::Stat;
use syscall::error::{Error, Result};
use syscall::error::{
    EACCES, EBADF, EBADFD, EINVAL, EIO, EISDIR, ENOENT, ENOMEM, ENOSYS, ENOTDIR, EOPNOTSUPP, EOVERFLOW,
    ESPIPE, ESRCH,
};
use syscall::flag::{
    MODE_CHR, MODE_DIR, O_ACCMODE, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR, O_STAT, O_WRONLY,
    SEEK_CUR, SEEK_END, SEEK_SET,
};
use syscall::io_uring::v1::{CqEntry64, IoUringSqeFlags, Priority, SqEntry64, StandardOpcode};
use syscall::io_uring::v1::operation::OpenFlags;
use syscall::io_uring::IoUringRecvInfo;
use syscall::scheme::{self, Scheme};

use redox_buffer_pool::BufferPoolOptions;

use redox_iou::executor::SpawnHandle;
use redox_iou::instance::ProducerInstance;
use redox_iou::reactor::SecondaryRingId;
use redox_iou::{memory::pool as redox_iou_pool, reactor};

use either::*;
use futures::StreamExt;
use once_cell::sync::OnceCell;

use crate::pci::{PciHeader, PciHeaderGeneral};

use crate::driver_interface::{PciAddress32, PcidOpcode};
use crate::driver_interface as pcid_interface;
use crate::{DeviceTree, Func, ResultExt, State as PcidState};

/// The PCI scheme, `pci:`.
///
/// # Organization
///
/// The top-level directory currently only contains a single entry, which is "bus". In the future,
/// we might have `pci:vbus/` or even `vbus/pci:` for abstracted bus enumeration (so that e.g. PCI
/// and USB use similar protocols).
///
/// Within `pci:bus`, the file tree follows the hierarchical format "bus/<bus number>/dev/<dev
/// number/func/<func number>/", where each intermediate directory contains the "info" dir, in
/// which single-file-single-value (think Linux's procfs or sysfs) key-value properties are
/// available.
///
/// The per-bus (as in `pci:bus/something/`), per-device, and per-function directories also all
/// have a `ctl` file, which when opened, provides functionality for the PCI specific io_uring
/// opcodes.
///
/// As per PCIe 3.0, bus numbers can be up to 8 bits; devices up to 5 bits; and functions up to 3
/// bits. All "numbers" are in hexademical with neither any "0x" prefix nor any "H" suffix. If PCI
/// Segment Groups (PCIe) are supported, bus numbers can also take the form `<4 hex-digits seg
/// group>-<regular bus number>`, where the seg group number is optional (otherwise simply the bus
/// number), and defaults to Segment Group 0 for backwards compatibility with plain PCI 3.0.
///
/// Device numbers are represented as two hex digits, but can only be in the `[0, 1f]` range (5
/// bits). Function numbers are hex digits, in the `[0, 7]` range (3 bits), so octal works fine
/// there as well.
///
/// There is also `pci:short/ADDRESS/`, where ADDRESS represents a function address in the format
/// `XX:YY.Z`, or `SSSS-XX:YY.Z`.
pub struct PcidScheme {
    spawn_handle: SpawnHandle,
    reactor_handle: reactor::Handle,
    self_weak: Option<Weak<Self>>,
    tree: Arc<RwLock<DeviceTree>>,
    state: Arc<PcidState>,

    file_handles: RwLock<BTreeMap<usize, Mutex<Handle>>>,
    next_handle: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SegmentGroupNum(pub u16);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BusNum {
    pub id: u8,
    pub seg: SegmentGroupNum,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceNum {
    pub bus: BusNum,
    pub id: u8,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FunctionNum {
    pub dev: DeviceNum,
    pub id: u8,
}
impl From<FunctionNum> for PciAddress32 {
    fn from(num: FunctionNum) -> Self {
        Self::default()
            .with_seg_group(num.dev.bus.seg.0)
            .with_bus(num.dev.bus.id)
            .with_device(num.dev.id)
            .with_function(num.id)
    }
}

// TODO
const TOPLEVEL_PCI_DIR: &str = "bus\n";
const BUS_DIR: &str = "ctl\ndev\n";
const DEV_DIR: &str = "ctl\nfunc\n";
// TODO: Add the info/ dir for key-value pairs represented as files.
const FUNC_DIR: &str = "ctl\n";
// TODO: *Maybe*, we should also allow the raw configuration space to be accessed as a file. The
// PCI Local Bus specification says that reserved fields can actually be written to, in which case
// they are no-ops to those bits. Reads to reserved fields return zero. We might consider having
// some validation, but otherwise it'd be really cool!

#[derive(Debug)]
struct List {
    offset: u64,
    data: OnceCell<Box<[u8]>>,
    kind: ListKind,
}
impl List {
    fn new(kind: ListKind) -> Self {
        Self {
            offset: 0,
            data: OnceCell::new(),
            kind,
        }
    }
    fn try_init(kind: &ListKind, scheme: &PcidScheme) -> Result<Box<[u8]>> {
        // TODO: Avoid allocation, by also allowing static references in List.
        fn from_static_str(s: &'static str) -> Box<[u8]> {
            s.as_bytes().to_vec().into_boxed_slice()
        }

        fn verify_bus_existence(tree: &DeviceTree, bus_num: BusNum) -> Result<()> {
            if !tree.busses.contains(&(bus_num.seg.0, bus_num.id)) {
                return Err(Error::new(ENOENT));
            }
            Ok(())
        }
        fn verify_dev_existence(tree: &DeviceTree, dev_num: DeviceNum) -> Result<()> {
            if !tree
                .devices
                .contains(&(dev_num.bus.seg.0, dev_num.bus.id, dev_num.id))
            {
                return Err(Error::new(ENOENT));
            }
            Ok(())
        }
        fn verify_func_existence(tree: &DeviceTree, func_num: FunctionNum) -> Result<()> {
            let key = PciAddress32::from(func_num);

            if !tree.functions.contains_key(&key) {
                return Err(Error::new(ENOENT));
            }
            Ok(())
        }

        Ok(match *kind {
            ListKind::TopLevel => from_static_str(TOPLEVEL_PCI_DIR),
            ListKind::Busses => {
                let tree_guard = scheme.tree.read().unwrap();
                Self::try_init_busses(&*tree_guard)?
            }
            ListKind::Bus(bus_num) => {
                verify_bus_existence(&*scheme.tree.read().unwrap(), bus_num)?;
                from_static_str(BUS_DIR)
            }
            ListKind::Devices(bus_num) => {
                let tree_guard = scheme.tree.read().unwrap();
                verify_bus_existence(&*tree_guard, bus_num)?;

                let range =
                    (bus_num.seg.0, bus_num.id, 0)..=(bus_num.seg.0, bus_num.id, u8::max_value());

                const LEN_PER_DEV: usize = "XX\n".len();

                let capacity = tree_guard.devices.range(range.clone()).count() * LEN_PER_DEV;
                let mut content = String::with_capacity(capacity);

                for (seg, bus, device_num) in tree_guard.devices.range(range).copied() {
                    debug_assert_eq!(seg, bus_num.seg.0);
                    debug_assert_eq!(bus, bus_num.id);

                    writeln!(content, "{:02x}", device_num).unwrap();
                }
                content.into_bytes().into_boxed_slice()
            }
            ListKind::Device(device_num) => {
                let tree_guard = scheme.tree.read().unwrap();
                verify_bus_existence(&*tree_guard, device_num.bus)?;
                verify_dev_existence(&*tree_guard, device_num)?;

                from_static_str(DEV_DIR)
            }
            ListKind::Functions(device_num) => {
                let tree_guard = scheme.tree.read().unwrap();
                verify_bus_existence(&*tree_guard, device_num.bus)?;
                verify_dev_existence(&*tree_guard, device_num)?;

                const LEN_PER_FUN: usize = "X\n".len();

                let range = {
                    let start = PciAddress32::default()
                        .with_seg_group(device_num.bus.seg.0)
                        .with_bus(device_num.bus.id)
                        .with_device(device_num.id)
                        .with_function(0);
                    let end = start.with_function(u8::max_value());

                    start..=end
                };

                let capacity = tree_guard.functions.range(range.clone()).count() * LEN_PER_FUN;
                let mut content = String::with_capacity(capacity);

                for (pciaddr32, _) in tree_guard.functions.range(range) {
                    writeln!(content, "{:01x}", pciaddr32.function()).unwrap();
                }

                content.into_bytes().into_boxed_slice()
            }
            ListKind::Function(func_num) => {
                let tree_guard = scheme.tree.read().unwrap();
                verify_bus_existence(&*tree_guard, func_num.dev.bus)?;
                verify_dev_existence(&*tree_guard, func_num.dev)?;
                verify_func_existence(&*tree_guard, func_num)?;

                from_static_str(FUNC_DIR)
            }
            ListKind::Short => {
                let tree_guard = scheme.tree.read().unwrap();

                let capacity = tree_guard.functions.keys().fold(0, |mut cap, addr| {
                    if tree_guard.uses_seg_groups {
                        cap += "SSSS-BB:DD.F".len();
                        if addr.seg_group() == 0{
                            cap += "BB:DD.F".len();
                        }
                    } else {
                        cap += "BB:DD.F".len();
                    }
                    cap
                });

                let mut content = String::with_capacity(capacity);

                for addr in tree_guard.functions.keys() {
                    if tree_guard.uses_seg_groups {
                        writeln!(content, "{seg:04x}-{bus:02x}:{dev:02x}.{func:01x}", seg=addr.seg_group(), bus=addr.bus(), dev=addr.device(), func=addr.function()).unwrap();
                        if addr.seg_group() == 0 {
                            writeln!(content, "{bus:02x}:{dev:02x}.{func:01x}", bus=addr.bus(), dev=addr.device(), func=addr.function()).unwrap();
                        }
                    } else {
                        writeln!(content, "{bus:02x}:{dev:02x}.{func:01x}", bus=addr.bus(), dev=addr.device(), func=addr.function()).unwrap();
                    }
                }
                content.into_bytes().into_boxed_slice()
            }
        })
    }
    fn try_init_busses(tree: &DeviceTree) -> Result<Box<[u8]>> {
        const LEN_PER_SEG_GRP_BUS: usize = "XXXX-YY\n".len();
        const LEN_PER_REG_BUS: usize = "YY\n".len();

        let content = if tree.uses_seg_groups {
            let with_seg_groups = &tree.busses;

            // This should be the exact capacity.
            let capacity = with_seg_groups.len() * LEN_PER_SEG_GRP_BUS
                + with_seg_groups.range((0, 0)..(1, 0)).count() * LEN_PER_REG_BUS;
            let mut content = String::with_capacity(capacity);

            for (seg_group, bus_num) in with_seg_groups.iter().copied() {
                writeln!(content, "{:04x}-{:02x}", seg_group, bus_num).unwrap();

                if seg_group == 0 {
                    writeln!(content, "{:02x}", bus_num).unwrap();
                }
            }
            content
        } else {
            let without_seg_groups = &tree.busses;

            // This should also be the exact capacity.
            let capacity = without_seg_groups.len() * LEN_PER_REG_BUS;
            let mut content = String::with_capacity(capacity);

            for (_, bus_num) in without_seg_groups.iter().copied() {
                writeln!(content, "{:02x}", bus_num).unwrap();
            }
            content
        };

        Ok(content.into_bytes().into_boxed_slice())
    }
    fn data(&self, scheme: &PcidScheme) -> Result<&[u8]> {
        self.data
            .get_or_try_init(|| Self::try_init(&self.kind, scheme))
            .map(|boxed_slice| &**boxed_slice)
    }
    fn inode(&self) -> u64 {
        use self::inode::*;

        const LIST: u64 = 1 << 60;

        match self.kind {
            // Count from 1 because inode 0 may be interpreted as invalid by some applications.
            ListKind::TopLevel => LIST | kind(1),
            ListKind::Busses => LIST | kind(2),
            ListKind::Bus(num) => LIST | kind(3) | seg(num.seg.0) | bus(num.id),
            ListKind::Devices(num) => LIST | kind(4) | seg(num.seg.0) | bus(num.id),
            ListKind::Device(num) => {
                LIST | kind(5) | seg(num.bus.seg.0) | bus(num.bus.id) | dev(num.id)
            }
            ListKind::Functions(num) => {
                LIST | kind(6) | seg(num.bus.seg.0) | bus(num.bus.id) | dev(num.id)
            }
            ListKind::Function(num) => {
                LIST | kind(7)
                    | seg(num.dev.bus.seg.0)
                    | bus(num.dev.bus.id)
                    | dev(num.dev.id)
                    | func(num.id)
            }
            ListKind::Short => LIST | kind(8),
        }
    }
}
mod inode {
    const KIND_SHIFT: u8 = 56;
    const FUNC_SHIFT: u8 = 0;
    const DEV_SHIFT: u8 = 8;
    const BUS_SHIFT: u8 = 16;
    const SEG_GROUP_SHIFT: u8 = 24;

    pub fn kind(kind: u8) -> u64 {
        u64::from(kind) << KIND_SHIFT
    }
    pub fn seg(seg: u16) -> u64 {
        u64::from(seg) << SEG_GROUP_SHIFT
    }
    pub fn bus(bus: u8) -> u64 {
        u64::from(bus) << BUS_SHIFT
    }
    pub fn dev(dev: u8) -> u64 {
        u64::from(dev) << DEV_SHIFT
    }
    pub fn func(func: u8) -> u64 {
        u64::from(func) << FUNC_SHIFT
    }
}
#[derive(Debug, Eq, PartialEq)]
enum ListKind {
    /// `pci:/`
    TopLevel,
    /// `pci:/bus/`
    Busses,
    /// `pci:/bus/XXXX-XX/` or `pci:/bus/XX/`
    Bus(BusNum),
    /// `pci:/bus/.../dev/`
    Devices(BusNum),
    /// `pci:/bus/.../dev/XX/`
    Device(DeviceNum),
    /// `pci:/bus/.../dev/../func/`
    Functions(DeviceNum),
    /// `pci:/bus/.../dev/../func/X`
    Function(FunctionNum),
    /// `pci:/short/SSSS-XX:YY.Z`, or `pci:short/XX:YY.Z`.
    Short,
}
#[derive(Debug)]
enum CtlSocket {
    Bus(BusNum),
    Device(DeviceNum),
    Function(FunctionNum),
}
impl CtlSocket {
    fn inode(&self) -> u64 {
        use self::inode::*;

        const CTL_SOCKET: u64 = 2 << 60;

        CTL_SOCKET
            | match self {
                Self::Bus(num) => kind(1) | seg(num.seg.0) | bus(num.id),
                Self::Device(num) => kind(1) | seg(num.bus.seg.0) | bus(num.bus.id) | dev(num.id),
                Self::Function(num) => {
                    kind(1)
                        | seg(num.dev.bus.seg.0)
                        | bus(num.dev.bus.id)
                        | dev(num.dev.id)
                        | func(num.id)
                }
            }
    }
}

#[derive(Debug)]
enum Handle {
    List(List),
    CtlSocket(CtlSocket),
    ReadConfigDir(u64, Vec<u8>),
}
impl Handle {
    fn list(kind: ListKind) -> Self {
        Self::List(List::new(kind))
    }
}

impl PcidScheme {
    pub fn new(
        spawn_handle: SpawnHandle,
        reactor_handle: reactor::Handle,
        tree: Arc<RwLock<DeviceTree>>,
        state: Arc<PcidState>,
    ) -> Arc<Self> {
        let mut self_arc = Arc::new(Self {
            spawn_handle,
            reactor_handle,
            self_weak: None,
            tree,
            state,
            file_handles: RwLock::new(BTreeMap::new()),
            next_handle: AtomicUsize::new(0),
        });
        let self_weak = Arc::downgrade(&self_arc);

        // SAFETY: This is safe because there is no active borrow of the inner data of self_arc;
        // all there is is a Weak, which does nothing but being moved within the scope of this function.
        unsafe { Arc::get_mut_unchecked(&mut self_arc) }.self_weak = Some(self_weak);

        self_arc
    }
    fn self_weak(&self) -> &Weak<Self> {
        self.self_weak
            .as_ref()
            .expect("expected PcidScheme to actually contain a self-ref after init")
    }
    fn self_arc(&self) -> Arc<Self> {
        self.self_weak()
            .upgrade()
            .expect("how would one get a ref to PciScheme if the Arc is dead?")
    }
    fn try_parse_bus_num(bus_num_str: &str) -> Option<BusNum> {
        if bus_num_str.len() == 2 {
            // Parses XX in `pci:/bus/XX/`

            let bus_num = u8::from_str_radix(bus_num_str, 16).ok()?;
            Some(BusNum {
                id: bus_num,
                seg: SegmentGroupNum::default(),
            })
        } else if bus_num_str.len() == 7 {
            // Parses XX and YYYY in `pci:/bus/YYYY-XX`.

            let seg_num_str = &bus_num_str[..4];
            if bus_num_str.chars().nth(4).map_or(false, |c| c != '-') {
                return None;
            }
            let bus_num_str = &bus_num_str[5..7];

            let bus_num = u8::from_str_radix(bus_num_str, 16).ok()?;
            let seg_num = u16::from_str_radix(seg_num_str, 16).ok()?;

            Some(BusNum {
                id: bus_num,
                seg: SegmentGroupNum(seg_num),
            })
        } else {
            None
        }
    }
    fn open_bus(&self, bus_num: BusNum, after_bus: &[&str], flags: usize) -> Result<Handle> {
        Ok(match *after_bus {
            [] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Bus(bus_num))
            }
            ["dev"] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Devices(bus_num))
            }
            ["dev", dev_num_str, ref after_dev @ ..] => self.open_dev(
                Self::try_parse_dev_num(bus_num, dev_num_str).ok_or(Error::new(ENOENT))?,
                after_dev,
                flags,
            )?,
            _ => return Err(Error::new(ENOENT)),
        })
    }
    fn try_parse_dev_num(bus: BusNum, dev_num_str: &str) -> Option<DeviceNum> {
        if dev_num_str.len() != 2 {
            return None;
        }
        Some(DeviceNum {
            bus,
            id: u8::from_str_radix(dev_num_str, 16).ok()?,
        })
    }
    fn open_dev(&self, dev_num: DeviceNum, after_dev: &[&str], flags: usize) -> Result<Handle> {
        Ok(match *after_dev {
            [] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Device(dev_num))
            }
            ["func"] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Functions(dev_num))
            }
            ["func", func_num_str, ref after_func @ ..] => self.open_func(
                Self::try_parse_func_num(dev_num, func_num_str).ok_or(Error::new(ENOENT))?,
                after_func,
                flags,
            )?,
            _ => return Err(Error::new(ENOENT)),
        })
    }
    fn try_parse_func_num(dev: DeviceNum, func_num_str: &str) -> Option<FunctionNum> {
        if func_num_str.len() == 1 {
            return None;
        }

        Some(FunctionNum {
            dev,
            id: u8::from_str_radix(func_num_str, 16).ok()?,
        })
    }
    fn open_func(
        &self,
        func_num: FunctionNum,
        after_func: &[&str],
        flags: usize,
    ) -> Result<Handle> {
        Ok(match *after_func {
            [] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Function(func_num))
            }
            ["ctl", ref rest @ ..] if !rest.is_empty() => return Err(Error::new(ENOTDIR)),
            ["ctl"] => {
                Self::validate_is_not_directory(flags)?;
                Handle::CtlSocket(CtlSocket::Function(func_num))
            }

            // TODO: Key-value pairs for things like vendor ID, device ID, vital product data,
            // whatever.
            ["info"] => return Err(Error::new(ENOENT)),

            _ => return Err(Error::new(ENOENT)),
        })
    }
    fn open_short(&self, after_short: &[&str], flags: usize) -> Result<Handle> {
        log::trace!("OPEN SHORT: `{:?}`", after_short);
        Ok(match *after_short {
            [] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;

                Handle::list(ListKind::Short)
            }
            [short_str, ref after_func @ ..] => {
                let function_num = Self::parse_short(short_str).ok_or(Error::new(ENOENT))?;

                return self.open_func(function_num, after_func, flags);
            }
        })
    }
    fn parse_short(short_str: &str) -> Option<FunctionNum> {
        // The short str takes one of the following formats:
        //
        // * SSSS-BB:DD.F
        // * BB:DD.F

        const LONG_LENGTH: usize = "SSSS-BB:DD.F".len();
        const SHORT_LENGTH: usize = "BB:DD.F".len();

        if short_str.len() != LONG_LENGTH && short_str.len() != SHORT_LENGTH {
            return None;
        }

        if short_str.len() == LONG_LENGTH {
            let seg_str = &short_str[..4];
            let dash_str = &short_str[4..5];
            let rest = &short_str[5..];

            if dash_str != "-" {
                return None;
            }
            let seg = u16::from_str_radix(seg_str, 16).ok()?;

            let mut partial = Self::parse_short(rest)?;
            partial.dev.bus.seg.0 = seg;
            return Some(partial);
        }

        let bus_str = &short_str[..2];
        let colon_str = &short_str[2..3];
        let dev_str = &short_str[3..5];
        let dot_str = &short_str[5..6];
        let func_str = &short_str[6..7];

        if colon_str != ":" || dot_str != "." {
            return None;
        }

        let bus = u8::from_str_radix(bus_str, 16).ok()?;
        let dev = u8::from_str_radix(dev_str, 16).ok()?;
        let func = u8::from_str_radix(func_str, 16).ok()?;

        let mut num = FunctionNum::default();
        num.id = func;
        num.dev.id = dev;
        num.dev.bus.id = bus;
        num.dev.bus.seg = Default::default();

        Some(num)
    }
    fn validate_is_directory(flags: usize) -> Result<()> {
        if flags & O_DIRECTORY != O_DIRECTORY && flags & O_STAT != O_STAT {
            return Err(Error::new(ENOTDIR));
        }
        Ok(())
    }
    fn validate_is_not_directory(flags: usize) -> Result<()> {
        if flags & O_DIRECTORY == O_DIRECTORY && flags & O_STAT != O_STAT {
            return Err(Error::new(EISDIR));
        }
        Ok(())
    }
    fn validate_is_rdonly(flags: usize) -> Result<()> {
        if flags & O_ACCMODE == O_WRONLY || flags & O_ACCMODE == O_RDWR {
            return Err(Error::new(EISDIR));
        }
        Ok(())
    }
}

impl Scheme for PcidScheme {
    fn open(&self, path: &[u8], flags: usize, uid: u32, gid: u32) -> Result<usize> {
        let path_str = str::from_utf8(path)
            .or(Err(Error::new(ENOENT)))?
            .trim_start_matches('/');
        log::debug!(
            "PCI SCHEME OPEN PATH=`{}` FLAGS={:#0x} uid={} gid={}",
            path_str,
            flags,
            uid,
            gid
        );

        // TODO: Don't heap allocate.
        let components = path_str.split('/').collect::<Vec<_>>();

        let handle = match *components {
            [] => {
                Self::validate_is_directory(flags).and_log_err_as_warn("EISDIR")?;
                Self::validate_is_rdonly(flags).and_log_err_as_warn("EISDIR RDONLY")?;
                Handle::list(ListKind::TopLevel)
            }
            ["short", ref after_short @ .. ] => {
                self.open_short(after_short, flags)?
            }
            ["read_config_dir"] => {
                if uid != 0 {
                    return Err(Error::new(EACCES));
                }
                Self::validate_is_not_directory(flags)?;
                // TODO: validate O_WRONLY
                Handle::ReadConfigDir(0, Vec::new())
            }
            ["bus"] => {
                Self::validate_is_directory(flags)?;
                Self::validate_is_rdonly(flags)?;
                Handle::list(ListKind::Busses)
            }
            ["bus", bus_num_str, ref after_bus @ ..] => self.open_bus(
                Self::try_parse_bus_num(bus_num_str).ok_or(Error::new(ENOENT))?,
                after_bus,
                flags,
            )?,
            _ => return Err(Error::new(ENOENT)),
        };

        let fd = self.next_handle.fetch_add(1, Ordering::Relaxed);

        log::trace!("New handle: {:?}, FD={}", handle, fd);

        let prev = self
            .file_handles
            .write()
            .unwrap()
            .insert(fd, Mutex::new(handle));
        if prev.is_some() {
            log::error!("Overwrote file handle {}", fd);
        }
        Ok(fd)
    }
    fn seek(&self, fd: usize, pos: isize, whence: usize) -> Result<isize> {
        log::debug!("PCI SCHEME SEEK FD={} POS={} whence={}", fd, pos, whence);

        let handles_guard = self.file_handles.read().unwrap();
        let mut handle = handles_guard
            .get(&fd)
            .ok_or(Error::new(EBADF))?
            .lock()
            .unwrap();

        let pos = i64::try_from(pos)?;

        match *handle {
            Handle::List(ref mut list) => {
                match whence {
                    SEEK_SET => list.offset = pos as u64,
                    SEEK_CUR => {
                        list.offset = if pos >= 0 {
                            list.offset
                                .checked_add(pos as u64)
                                .ok_or(Error::new(EOVERFLOW))?
                        } else {
                            list.offset - (-pos) as u64
                        }
                    }
                    // TODO
                    SEEK_END => {
                        let len = u64::try_from(list.data(self)?.len())?;

                        if pos > 0 {
                            list.offset =
                                len.checked_add(pos as u64).ok_or(Error::new(EOVERFLOW))?;
                        } else {
                            list.offset = len
                                .checked_sub((-pos) as u64)
                                .ok_or(Error::new(EOVERFLOW))?;
                        }
                    }
                    _ => return Err(Error::new(EINVAL)),
                }
                Ok(isize::try_from(list.offset)?)
            }
            Handle::CtlSocket(_) => return Err(Error::new(ESPIPE)),
            Handle::ReadConfigDir(ref mut offset, _) => {
                match whence {
                    SEEK_SET => *offset = pos as u64,
                    SEEK_CUR => {
                        if pos > 0 {
                            *offset += pos as u64;
                        } else {
                            *offset = offset
                                .checked_sub((-pos) as u64)
                                .ok_or(Error::new(EINVAL))?;
                        }
                    }
                    SEEK_END | _ => return Err(Error::new(ESPIPE)),
                }
                Ok(isize::try_from(*offset)?)
            }
        }
    }
    fn read(&self, id: usize, buf: &mut [u8]) -> Result<usize> {
        log::debug!(
            "PCI SCHEME READ FD={} BUF=<AT {:p} LEN {}>",
            id,
            buf.as_mut_ptr(),
            buf.len()
        );

        let handles_guard = self.file_handles.read().unwrap();
        let mut handle = handles_guard
            .get(&id)
            .ok_or(Error::new(EBADF))?
            .lock()
            .unwrap();

        match *handle {
            Handle::List(ref mut list) => {
                let offset = match usize::try_from(list.offset) {
                    Ok(o) => o,
                    Err(_) => return Ok(0),
                };

                let data = list.data(self)?;
                log::trace!("LIST HANDLE: {:?}", list);

                let start_buf_offset = cmp::min(offset, data.len());
                let end_buf_offset = cmp::min(
                    start_buf_offset
                        .checked_add(buf.len())
                        .ok_or(Error::new(EOVERFLOW))?,
                    data.len(),
                );
                let src_buf = &data[start_buf_offset..end_buf_offset];

                let bytes_to_read = end_buf_offset - start_buf_offset;
                let bytes_to_read_u64 = u64::try_from(bytes_to_read)?;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
                list.offset = list
                    .offset
                    .checked_add(bytes_to_read_u64)
                    .ok_or(Error::new(EOVERFLOW))?;
                Ok(bytes_to_read)
            }
            Handle::CtlSocket(_) => Err(Error::new(EBADF)),
            Handle::ReadConfigDir(_, _) => Err(Error::new(EBADF)),
        }
    }
    fn fpath(&self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        log::debug!(
            "PCI SCHEME FPATH FD={}, BUF=<AT {:p} LEN {}>",
            fd,
            buf.as_ptr(),
            buf.len()
        );

        let handles_guard = self.file_handles.read().unwrap();
        let handle = handles_guard
            .get(&fd)
            .ok_or(Error::new(EBADF))?
            .lock()
            .unwrap();

        let mut cursor = io::Cursor::new(buf);
        write!(cursor, "pci:").unwrap();

        fn write_bus(mut sink: impl io::Write, bus_num: BusNum, uses_seg_groups: bool) {
            if uses_seg_groups {
                write!(sink, "bus/{:04x}-{:02x}", bus_num.seg.0, bus_num.id).unwrap();
            } else {
                write!(sink, "bus/{:02x}", bus_num.id).unwrap();
            }
        }

        let uses_seg_groups = self.tree.read().unwrap().uses_seg_groups;

        match *handle {
            Handle::List(ref list) => match list.kind {
                ListKind::TopLevel => (),
                ListKind::Busses => write!(cursor, "bus").unwrap(),
                ListKind::Bus(bus_num) => write_bus(&mut cursor, bus_num, uses_seg_groups),
                ListKind::Devices(bus_num) => {
                    write_bus(&mut cursor, bus_num, uses_seg_groups);
                    write!(cursor, "/dev").unwrap();
                }
                ListKind::Device(dev_num) => {
                    write_bus(&mut cursor, dev_num.bus, uses_seg_groups);
                    write!(cursor, "/dev/{:02x}", dev_num.id).unwrap();
                }
                ListKind::Functions(dev_num) => {
                    write_bus(&mut cursor, dev_num.bus, uses_seg_groups);
                    write!(cursor, "/dev/{:02x}/func", dev_num.id).unwrap();
                }
                ListKind::Function(func_num) => {
                    write_bus(&mut cursor, func_num.dev.bus, uses_seg_groups);
                    write!(
                        cursor,
                        "/dev/{:02x}/func/{:02x}",
                        func_num.dev.id, func_num.id
                    )
                    .unwrap();
                }
                ListKind::Short => write!(cursor, "/short").unwrap(),
            },
            Handle::CtlSocket(ref socket) => match *socket {
                CtlSocket::Bus(bus_num) => {
                    write_bus(&mut cursor, bus_num, uses_seg_groups);
                    write!(cursor, "/ctl").unwrap();
                }
                CtlSocket::Device(dev_num) => {
                    write_bus(&mut cursor, dev_num.bus, uses_seg_groups);
                    write!(cursor, "/dev/{:02x}/ctl", dev_num.id).unwrap();
                }
                CtlSocket::Function(func_num) => {
                    write_bus(&mut cursor, func_num.dev.bus, uses_seg_groups);
                    write!(
                        cursor,
                        "/dev/{:02x}/func/{:01x}/ctl",
                        func_num.dev.id, func_num.id
                    )
                    .unwrap();
                }
            },
            Handle::ReadConfigDir(_, _) => write!(cursor, "/read_config_dir").unwrap(),
        }

        Ok(cursor.position().try_into()?)
    }
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize> {
        log::debug!(
            "PCI SCHEME WRITE FD={}, BUF=<AT {:p} LEN {}>",
            fd,
            buf.as_ptr(),
            buf.len()
        );

        let handles_guard = self.file_handles.read().unwrap();
        let mut handle = handles_guard
            .get(&fd)
            .ok_or(Error::new(EBADF))?
            .lock()
            .or(Err(Error::new(EBADFD)))?;

        match *handle {
            Handle::ReadConfigDir(ref mut offset, ref mut data) => {
                data.try_reserve_exact(buf.len())
                    .or(Err(Error::new(ENOMEM)))?;
                data.extend(buf);
                Ok(buf.len())
            }
            Handle::CtlSocket(_) | Handle::List(_) => return Err(Error::new(EBADF)),
        }
    }
    fn fstat(&self, fd: usize, stat: &mut Stat) -> Result<usize> {
        log::debug!(
            "PCI SCHEME FSTAT FD={}, STAT=<`Stat` AT {:p}>",
            fd,
            stat as *mut Stat
        );

        let handles_guard = self.file_handles.read().unwrap();
        let handle = handles_guard
            .get(&fd)
            .ok_or(Error::new(EBADF))?
            .lock()
            .unwrap();

        match *handle {
            Handle::List(ref list) => {
                let data = list.data(self).and_log_err_as_warn("list.data failed")?;
                let size = u64::try_from(data.len())?;
                log::trace!("LIST: {:?}", list);

                const BLKSZ: u32 = 4096;

                *stat = Stat {
                    st_dev: 0,
                    st_blksize: BLKSZ,
                    st_blocks: (size + u64::from(BLKSZ) - 1) / u64::from(BLKSZ),
                    st_size: size,

                    // FIXME: Somehow directories with inodes (maybe because of the higher bits
                    // here?) behave weirdly when listing.
                    st_ino: 0, // list.inode(),

                    st_mode: MODE_DIR,
                    st_nlink: if list.kind == ListKind::TopLevel {
                        1
                    } else {
                        2
                    },

                    // TODO: PCI user and group
                    st_uid: 0,
                    st_gid: 0,

                    st_atime: 0,      // TODO
                    st_atime_nsec: 0, // TODO
                    st_ctime: 0,      // TODO
                    st_ctime_nsec: 0, // TODO
                    st_mtime: 0,      // TODO
                    st_mtime_nsec: 0, // TODO
                };
                log::trace!("NEW STAT: {:?}", stat);
            }
            Handle::CtlSocket(ref socket) => {
                *stat = Stat {
                    st_dev: 0,
                    st_blksize: 4096,
                    st_blocks: 0,
                    st_size: 0,

                    // FIXME
                    st_ino: 0, //socket.inode(),

                    st_mode: MODE_CHR | 0o000,
                    st_nlink: 1,

                    // TODO
                    st_uid: 0,
                    st_gid: 0,

                    st_atime: 0,      // TODO
                    st_atime_nsec: 0, // TODO
                    st_ctime: 0,      // TODO
                    st_ctime_nsec: 0, // TODO
                    st_mtime: 0,      // TODO
                    st_mtime_nsec: 0, // TODO
                }
            }
            Handle::ReadConfigDir(_, _) => {
                stat.st_mode = MODE_CHR;
            }
        }
        Ok(0)
    }

    fn recv_io_uring(&self, ctx: scheme::Ctx, info: &IoUringRecvInfo) -> Result<usize> {
        log::debug!(
            "PCI SCHEME RECV_IOURING CTX=<PID={pid} UID={uid} GID={gid}> VERSION={major}.{minor}.{patch}",
            pid=ctx.pid, uid=ctx.uid, gid=ctx.gid, major=info.version.major,
            minor=info.version.minor, patch=info.version.patch);

        let instance = ProducerInstance::new(info)
            .and_log_err_as_warn("failed to create producer instance")?;

        let reactor_handle = self.reactor_handle.clone();
        let ring = reactor_handle
            .reactor()
            .add_producer_instance(instance, Priority::default())
            .and_log_err_as_warn("failed to register producer instance to reactor")?;
        let mut stream = reactor_handle.producer_sqes(ring, 64);

        let this = self.self_arc();

        self.spawn_handle.spawn(async move {
            log::trace!("Spawning works");
            let mut pool = None;

            while let Some(sqe) = stream.next().await {
                log::debug!("PCI SCHEME RECV SQE {:?}", sqe);
                let sqe = match sqe {
                    Ok(sqe) => sqe,
                    Err(error) => {
                        log::warn!("Failed to receive SQE: {}, dropping that ring.", error);
                        return;
                    }
                };

                let cqe = if let Some(standard_opcode) = StandardOpcode::from_raw(sqe.opcode) {
                    let cqe_res = this.handle_standard_opcode(&ctx, &reactor_handle, ring, &mut pool, standard_opcode, &sqe).await;
                    Self::or_error(&sqe, cqe_res, 0)
                } else if let Some(pcid_opcode) = PcidOpcode::from_raw(sqe.opcode) {
                    let cqe_res = this
                        .handle_pcid_opcode(
                            &ctx,
                            &reactor_handle,
                            ring,
                            &mut pool,
                            pcid_opcode,
                            &sqe,
                        )
                        .await;
                    Self::or_error(&sqe, cqe_res, 0)
                } else {
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
    fn close(&self, fd: usize) -> Result<usize> {
        log::debug!("PCI SCHEME CLOSE FD={}", fd);

        let mut handles_guard = self.file_handles.write().unwrap();

        match handles_guard
            .remove(&fd)
            .ok_or(Error::new(EBADF))?
            .into_inner()
            .or(Err(Error::new(EBADFD)))?
        {
            Handle::ReadConfigDir(_, ref data) => {
                let os_str = OsStr::from_bytes(&data);
                let mut config = crate::config::Config::default();
                crate::load_config_dir(os_str, &mut config);
                log::debug!("beginning to process config from user request...");
                crate::process_config(&config, &*self.tree.read().unwrap(), &self.state);
                log::debug!("finished processing config from user request");
            }
            Handle::CtlSocket(_) => (),
            Handle::List(_) => (),
        }

        Ok(0)
    }
}
async fn get_or_init_pool<'a>(pool: &'a mut Option<redox_iou::memory::BufferPool>, reactor: &reactor::Handle, ring: SecondaryRingId) -> Result<&'a mut redox_iou::memory::BufferPool> {
    Ok(match pool {
        Some(p) => p,
        None => {
            log::debug!("Creating producer pool");
            let new_pool = reactor
                .create_producer_buffer_pool(ring, Priority::default())
                .await?
                .with_options(
                    BufferPoolOptions::new()
                        .with_minimum_alignment(1)
                );
            *pool = Some(new_pool);
            pool.as_mut().unwrap()
        }
    })
}
impl PcidScheme {
    // TODO: Make it possible to get an Arc of the driver, to avoid lifetime errors with the
    // executor.
    pub async fn handle_standard_opcode(
        &self,
        ctx: &scheme::Ctx,
        reactor: &reactor::Handle,
        ring: SecondaryRingId,
        pool: &mut Option<redox_iou::memory::BufferPool>,
        opcode: StandardOpcode,
        sqe: &SqEntry64,
    ) -> Result<CqEntry64> {
        match opcode {
            StandardOpcode::Open => {
                log::trace!("Handle Open");
                let pool = get_or_init_pool(pool, reactor, ring).await?;
                log::trace!("Created pool; now doing to basic validation of args");

                let flags = OpenFlags::from_bits(sqe.syscall_flags).ok_or(Error::new(EINVAL))?;

                let len = u32::try_from(sqe.len)?;
                let align = 1;
                let offset = u32::try_from(sqe.addr)?;
                let flags = usize::try_from(sqe.offset)?;

                log::trace!("Validating, acquiring a borrowed slice");

                let slice = pool.acquire_borrowed_slice::<redox_buffer_pool::NoGuard>(len, align, redox_buffer_pool::AllocationStrategy::Fixed(offset)).ok_or(Error::new(ENOMEM))?;

                log::trace!("Acquired, at {} len {} mmap {} mmap len {} extra {:?}", slice.offset(), slice.len(), slice.mmap_offset(), slice.mmap_size(), slice.extra());
                log::trace!("Slice: LEN = {} DATA = {:?}", slice.as_slice().len(), slice.as_slice());
                let fd = self.open(&*slice, flags, ctx.uid, ctx.gid)?;
                let fd64 = u64::try_from(fd)?;
                log::trace!("Opened");

                Ok(CqEntry64  {
                    user_data: sqe.user_data,
                    flags: 0, // TODO
                    extra: 0,
                    status: Error::mux64(Ok(fd64)),
                })
            },
            _ => {
                log::warn!("TODO: handle standard opcode {:?}, sqe {:?}", opcode, sqe);
                return Err(Error::new(ENOSYS));
            }
        }
    }
    pub async fn handle_pcid_opcode(
        &self,
        _ctx: &scheme::Ctx,
        reactor: &reactor::Handle,
        ring: SecondaryRingId,
        pool: &mut Option<redox_iou::memory::BufferPool>,
        opcode: PcidOpcode,
        sqe: &SqEntry64,
    ) -> Result<CqEntry64> {
        let pool = get_or_init_pool(pool, reactor, ring).await?;

        fn check_if_version(sqe: &SqEntry64) -> Result<()> {
            let pcid_if_version = sqe.syscall_flags;
            if pcid_if_version != 1 {
                return Err(Error::new(ENOSYS));
            }
            Ok(())
        }

        let function_num = {
            let fd = usize::try_from(sqe.fd).or(Err(Error::new(EBADF)))?;

            let file_handles = self.file_handles.read().or(Err(Error::new(EBADFD)))?;
            let file_handle_lock = file_handles.get(&fd).ok_or(Error::new(EBADF))?;
            let file_handle = file_handle_lock.lock().or(Err(Error::new(EBADFD)))?;

            match *file_handle {
                Handle::CtlSocket(CtlSocket::Function(func)) => func,
                Handle::CtlSocket(_) => return Err(Error::new(ENOSYS)),
                Handle::List(_) | Handle::ReadConfigDir(_, _) => return Err(Error::new(EBADF)),
            }
        };
        let function_lock = {
            let device_tree = self.tree.read().or(Err(Error::new(EBADFD)))?;

            let function_lock = device_tree
                .functions
                .get(&PciAddress32::from(function_num))
                .ok_or(Error::new(EBADF))?;

            Arc::clone(function_lock)
        };

        match opcode {
            PcidOpcode::FetchConfig => {
                check_if_version(sqe)?;
                let function = function_lock.read().or(Err(Error::new(EBADFD)))?;
                log::debug!("Fetching config for function {:?} at {:02x}:{:02x}.{:01x}", function, function_num.dev.bus.id, function_num.dev.id, function_num.id);
                let len = u32::try_from(sqe.len)?;
                let alignment = u32::try_from(mem::align_of::<PciHeaderGeneral>()).unwrap();
                let addr = u32::try_from(sqe.addr)?;

                let bytes_to_copy = mem::size_of::<pcid_interface::SubdriverArguments>();
                if usize::try_from(sqe.len).unwrap() < bytes_to_copy {
                    return Err(Error::new(EINVAL));
                }


                let mut slice = pool.acquire_borrowed_slice::<redox_buffer_pool::NoGuard>(len, alignment, redox_buffer_pool::AllocationStrategy::Fixed(addr)).ok_or(Error::new(ENOMEM))?;
                log::trace!("Slice to use at {}, len {}, mmap start {}, mmap size {}, extra {:?}", slice.offset(), slice.len(), slice.mmap_offset(), slice.mmap_size(), slice.extra());

                {
                    let function_guard = function_lock.read().or(Err(Error::new(EBADFD)))?;

                    let header = match function_guard.header {
                        PciHeader::General(ref g) => g,
                        PciHeader::PciToPci(_) => return Err(Error::new(EINVAL)),
                    };
                    // TODO: Don't use this simple struct, but the actual header (maybe even as a
                    // regular file), with some extra information not present in the configuration
                    // space, such as the PCI address (bus-dev-func).
                    *plain::from_mut_bytes(&mut slice[..bytes_to_copy]).unwrap() = pcid_interface::SubdriverArguments {
                        func: pcid_interface::PciFunction {
                            bus_num: function_num.dev.bus.id,
                            dev_num: function_num.dev.id,
                            func_num: function_num.id,

                            bars: header.bars,
                            bar_sizes: function_guard.bar_sizes,
                            venid: header.header_base.vendor_id,
                            devid: header.header_base.device_id,

                            legacy_interrupt_line: header.interrupt_line,
                            legacy_interrupt_pin: header.interrupt_pin,
                        },
                    };
                }
                Ok(CqEntry64 {
                    user_data: sqe.user_data,
                    status: Error::mux64(Ok(0)),
                    // TODO
                    flags: 0,
                    extra: 0,
                })
            }
            PcidOpcode::FetchAllCapabilities => {
                check_if_version(sqe)?;

                let function = function_lock.read().or(Err(Error::new(EBADFD)))?;

                let len = u32::try_from(sqe.len)?;
                let alignment = u32::try_from(mem::align_of::<pcid_interface::CapabilityRawTagged>())?;
                let addr = u32::try_from(sqe.offset)?;

                let index = sqe.addr;

                let readable_caps = u64::try_from(function.pci_capabilities.len() + function.pcie_capabilities.len())?;
                let cap_size = u32::try_from(mem::size_of::<pcid_interface::CapabilityRawTagged>())?;
                let caps_to_read = usize::try_from(len)?;

                if index >= readable_caps {
                    return Ok(CqEntry64 {
                        user_data: sqe.user_data,
                        status: Error::mux64(Ok(0)),
                        // TODO
                        flags: 0,
                        extra: 0,
                    })
                }

                let mut caps_read = 0u32;

                fn pci_to_raw(pci: pcid_interface::PciCapabilityRawTagged) -> pcid_interface::CapabilityRawTagged {
                    pcid_interface::CapabilityRawTagged {
                        kind: pcid_interface::CapabilityKind::Pci as u8,
                        raw: pcid_interface::CapabilityRaw {
                            pci,
                        },
                    }
                }
                fn pcie_to_raw(pcie: pcid_interface::PcieCapabilityRawTagged) -> pcid_interface::CapabilityRawTagged {
                    pcid_interface::CapabilityRawTagged {
                        kind: pcid_interface::CapabilityKind::Pcie as u8,
                        raw: pcid_interface::CapabilityRaw {
                            pcie,
                        },
                    }
                }

                log::trace!("inserting capabilities with params len {} align {} addr {} start {} count {} caps readable {}", len, alignment, addr, index, caps_to_read, readable_caps);

                let index = usize::try_from(index)?;
                for capability in function.pci_capabilities.iter().map(|(_, c)| pci_to_raw(c.to_raw())).chain(function.pcie_capabilities.iter().map(|(_, c)| pcie_to_raw(c.to_raw()))).skip(index).take(caps_to_read) {
                    log::trace!("pool: {:?}", pool);
                    let offset = addr + caps_read * cap_size;
                    log::trace!("trying to allocate {} bytes with alignment {}, at {}", cap_size, alignment, offset);
                    let mut slice = pool.acquire_borrowed_slice::<redox_buffer_pool::NoGuard>(cap_size, alignment, redox_iou_pool::AllocationStrategy::Fixed(offset)).ok_or(Error::new(ENOMEM))?;
                    log::trace!("slice for cap at {} len {} mmap_offset {} mmap_size {} extra {:?}", slice.offset(), slice.len(), slice.mmap_offset(), slice.mmap_size(), slice.extra());
                    *plain::from_mut_bytes::<pcid_interface::CapabilityRawTagged>(&mut *slice).unwrap() = capability;
                    log::trace!("plain good");
                    caps_read += 1;
                    log::trace!("caps_read: {}", caps_read);
                }

                log::trace!("caps read: {}", caps_read);
                log::trace!("caps left to read: {}", readable_caps - u64::from(caps_read));

                Ok(CqEntry64 {
                    user_data: sqe.user_data,
                    status: Error::mux64(Ok(caps_read.into())),
                    extra: readable_caps - u64::from(caps_read),
                    // TODO
                    flags: 0,
                })
            }
            PcidOpcode::GetCapability => {
                log::warn!("TODO: GetCapability");
                check_if_version(sqe)?;
                Err(Error::new(ENOSYS))
            }
            PcidOpcode::SetCapability => {
                use self::pcid_interface::SetCapabilityInfoRaw;

                check_if_version(sqe)?;

                let offset = u32::try_from(sqe.offset)?;
                let len = u32::try_from(sqe.len)?;

                // TODO: Currently pcid assumes that there is only one capability for each type.
                // However, I haven't been able to confirm that the PCI(e) spec limits that. Rather
                // than finding the first capability with the matching type, we should instead
                // check that the type is correct and operate on a capability based on index.
                let _capability_index = u32::try_from(sqe.addr)?;

                let struct_len = u32::try_from(mem::size_of::<SetCapabilityInfoRaw>())?;
                let struct_align = u32::try_from(mem::align_of::<SetCapabilityInfoRaw>())?;

                if len < struct_len {
                    return Err(Error::new(EINVAL));
                }

                let slice = pool.acquire_borrowed_slice::<redox_buffer_pool::NoGuard>(struct_len, struct_align, redox_iou_pool::AllocationStrategy::Fixed(offset)).ok_or(Error::new(ENOMEM))?;

                let info = plain::from_bytes::<SetCapabilityInfoRaw>(&*slice).unwrap();
                let set_capability_info = pcid_interface::SetCapabilityInfo::construct(*info).ok_or(Error::new(EINVAL))?;

                let mut func = function_lock.write().or(Err(Error::new(EBADFD)))?;
                let result = crate::set_capability_info(&mut *func, &set_capability_info, function_num.into(), self.state.preferred_cfg_access());

                match result {
                    Ok(()) => Ok(CqEntry64 {
                        user_data: sqe.user_data,
                        extra: 0,
                        // TODO
                        flags: 0,
                        status: Error::mux64(Ok(0)),
                    }),
                    Err(pcid_interface::PcidServerResponseError::InvalidBitPattern) => Err(Error::new(EINVAL)),
                    Err(pcid_interface::PcidServerResponseError::NonexistentCapability(_)) => Err(Error::new(ENOENT)),
                    Err(_) => Err(Error::new(EIO)),
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_short() {

        macro_rules! function_num(
            ($bus:literal : $dev:literal . $func:literal) => {{
                function_num!(0x0000 - $bus : $dev . $func)
            }};
            ($seg:literal - $bus:literal : $dev:literal . $func:literal) => {{
                let mut func = FunctionNum::default();
                func.id = $func;
                func.dev.id = $dev;
                func.dev.bus.id = $bus;
                func.dev.bus.seg.0 = $seg;
                func
            }};
        );

        assert_eq!(PcidScheme::parse_short("0000-01:1f.2"), Some(function_num!(0x0000 - 0x01 : 0x1f . 0x2)));
        assert_eq!(PcidScheme::parse_short("DEAD-BE:EF.0"), Some(function_num!(0xdead - 0xbe : 0xef . 0)));
        assert_eq!(PcidScheme::parse_short("BE:EF.0"), Some(function_num!(0x0000 - 0xbe : 0xef . 0)));
        assert_eq!(PcidScheme::parse_short("PCI is good!"), None);
        assert_eq!(PcidScheme::parse_short("0000 01 1f 2"), None);
    }
}
