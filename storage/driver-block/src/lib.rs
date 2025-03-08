use std::cmp;
use std::io::{self, Read, Seek, SeekFrom};

use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fmt::Write;
use std::str;

use libredox::Fd;
use partitionlib::{LogicalBlockSize, PartitionTable};
use redox_scheme::{
    CallRequest, CallerCtx, OpenResult, RequestKind, Response, SchemeBlock, SignalBehavior, Socket,
};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, Stat, EACCES, EAGAIN, EBADF, EISDIR, ENOENT, ENOLCK, EOPNOTSUPP, EOVERFLOW,
    MODE_DIR, MODE_FILE, O_DIRECTORY, O_STAT,
};

/// Split the read operation into a series of block reads.
/// `read_fn` will be called with a block number to be read, and a buffer to be filled.
/// `read_fn` must return a full block of data.
/// Result will be the number of bytes read.
fn block_read(
    offset: u64,
    blksize: u32,
    buf: &mut [u8],
    mut read_fn: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
) -> io::Result<usize> {
    // TODO: Yield sometimes, perhaps after a few blocks or something.

    if buf.len() == 0 {
        return Ok(0);
    }
    let to_copy = usize::try_from(
        offset.saturating_add(u64::try_from(buf.len()).expect("buf.len() larger than u64"))
            - offset,
    )
    .expect("bytes to copy larger than usize");
    let mut curr_buf = &mut buf[..to_copy];
    let mut curr_offset = offset;
    let blk_size = usize::try_from(blksize).expect("blksize larger than usize");
    let mut total_read = 0;

    let mut block_bytes = [0u8; 4096];
    let block_bytes = &mut block_bytes[..blk_size];

    while curr_buf.len() > 0 {
        // TODO: Async/await? I mean, shouldn't AHCI be async?

        let blk_offset =
            usize::try_from(curr_offset % u64::from(blksize)).expect("usize smaller than blksize");
        let to_copy = cmp::min(curr_buf.len(), blk_size - blk_offset);
        assert!(blk_offset + to_copy <= blk_size);

        read_fn(curr_offset / u64::from(blksize), block_bytes)?;

        let src_buf = &block_bytes[blk_offset..];

        curr_buf[..to_copy].copy_from_slice(&src_buf[..to_copy]);
        curr_buf = &mut curr_buf[to_copy..];
        curr_offset += u64::try_from(to_copy).expect("bytes to copy larger than u64");
        total_read += to_copy;
    }
    Ok(total_read)
}

pub trait Disk {
    fn block_size(&self) -> u32;
    fn size(&self) -> u64;

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<Option<usize>>;
    fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<Option<usize>>;
}

impl<T: Disk + ?Sized> Disk for Box<T> {
    fn block_size(&self) -> u32 {
        (**self).block_size()
    }

    fn size(&self) -> u64 {
        (**self).size()
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<Option<usize>> {
        (**self).read(block, buffer)
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<Option<usize>> {
        (**self).write(block, buffer)
    }
}

pub struct DiskWrapper<T> {
    pub disk: T,
    pub pt: Option<PartitionTable>,
}

impl<T: Disk> DiskWrapper<T> {
    pub fn pt(disk: &mut T) -> Option<PartitionTable> {
        let bs = match disk.block_size() {
            512 => LogicalBlockSize::Lb512,
            4096 => LogicalBlockSize::Lb4096,
            _ => return None,
        };
        struct Device<'a> {
            disk: &'a mut dyn Disk,
            offset: u64,
        }

        impl<'a> Seek for Device<'a> {
            fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
                let size = i64::try_from(self.disk.size()).or(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Disk larger than 2^63 - 1 bytes",
                )))?;

                self.offset = match from {
                    SeekFrom::Start(new_pos) => cmp::min(self.disk.size(), new_pos),
                    SeekFrom::Current(new_pos) => {
                        cmp::max(0, cmp::min(size, self.offset as i64 + new_pos)) as u64
                    }
                    SeekFrom::End(new_pos) => cmp::max(0, cmp::min(size + new_pos, size)) as u64,
                };

                Ok(self.offset)
            }
        }
        // TODO: Perhaps this impl should be used in the rest of the scheme.
        impl<'a> Read for Device<'a> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let blksize = self.disk.block_size();
                let size_in_blocks = self.disk.size() / u64::from(blksize);

                let disk = &mut self.disk;

                let read_block = |block: u64, block_bytes: &mut [u8]| {
                    if block >= size_in_blocks {
                        return Err(io::Error::from_raw_os_error(syscall::EOVERFLOW));
                    }
                    loop {
                        match disk.read(block, block_bytes) {
                            Ok(Some(bytes)) => {
                                assert_eq!(bytes, block_bytes.len());
                                return Ok(());
                            }
                            Ok(None) => {
                                std::thread::yield_now();
                                continue;
                            }
                            Err(err) => return Err(io::Error::from_raw_os_error(err.errno)),
                        }
                    }
                };
                let bytes_read = block_read(self.offset, blksize, buf, read_block)?;

                self.offset += bytes_read as u64;
                Ok(bytes_read)
            }
        }

        partitionlib::get_partitions(&mut Device { disk, offset: 0 }, bs)
            .ok()
            .flatten()
    }

    pub fn new(mut disk: T) -> Self {
        Self {
            pt: Self::pt(&mut disk),
            disk,
        }
    }

    pub fn disk(&self) -> &T {
        &self.disk
    }

    pub fn disk_mut(&mut self) -> &mut T {
        &mut self.disk
    }

    pub fn block_size(&self) -> u32 {
        self.disk.block_size()
    }

    pub fn size(&self) -> u64 {
        self.disk.size()
    }

    pub fn read(
        &mut self,
        part_num: Option<usize>,
        block: u64,
        buf: &mut [u8],
    ) -> syscall::Result<Option<usize>> {
        if let Some(part_num) = part_num {
            let part = self
                .pt
                .as_ref()
                .ok_or(syscall::Error::new(EBADF))?
                .partitions
                .get(part_num)
                .ok_or(syscall::Error::new(EBADF))?;

            let block_size = u64::from(self.block_size());
            if block >= part.size / block_size {
                return Err(syscall::Error::new(EOVERFLOW));
            }

            let abs_block = part.start_lba + block;

            self.disk.read(abs_block, buf)
        } else {
            self.disk.read(block, buf)
        }
    }

    pub fn write(
        &mut self,
        part_num: Option<usize>,
        block: u64,
        buf: &[u8],
    ) -> syscall::Result<Option<usize>> {
        if let Some(part_num) = part_num {
            let part = self
                .pt
                .as_ref()
                .ok_or(syscall::Error::new(EBADF))?
                .partitions
                .get(part_num)
                .ok_or(syscall::Error::new(EBADF))?;

            let block_size = u64::from(self.block_size());
            if block >= part.size / block_size {
                return Err(syscall::Error::new(EOVERFLOW));
            }

            let abs_block = part.start_lba + block;

            self.disk.write(abs_block, buf)
        } else {
            self.disk.write(block, buf)
        }
    }
}

enum Handle {
    List(Vec<u8>),       // entries
    Disk(u32),           // disk num
    Partition(u32, u32), // disk num, part num
}

pub struct DiskScheme<T> {
    scheme_name: String,
    socket: Socket,
    disks: BTreeMap<u32, DiskWrapper<T>>,
    handles: BTreeMap<usize, Handle>,
    next_id: usize,
    blocked: Vec<CallRequest>,
}

impl<T: Disk> DiskScheme<T> {
    pub fn new(scheme_name: String, disks: BTreeMap<u32, T>) -> Self {
        assert!(scheme_name.starts_with("disk"));
        let socket = Socket::nonblock(&scheme_name).expect("failed to create disk scheme");

        Self {
            scheme_name,
            socket,
            disks: disks
                .into_iter()
                .map(|(k, disk)| (k, DiskWrapper::new(disk)))
                .collect(),
            next_id: 0,
            handles: BTreeMap::new(),
            blocked: vec![],
        }
    }

    pub fn event_handle(&self) -> &Fd {
        self.socket.inner()
    }

    /// Process pending and new requests.
    ///
    /// This needs to be called each time there is a new event on the scheme
    /// file and each time a read or write operation has completed.
    // FIXME maybe split into one method for events on the scheme fd and one
    // to call when an irq is received to indicate that blocked packets can
    // be processed.
    pub fn tick(&mut self) -> io::Result<()> {
        // Handle any blocked requests
        let mut i = 0;
        while i < self.blocked.len() {
            if let Some(resp) = self.blocked[i].handle_scheme_block(self) {
                self.socket
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("driver-block: failed to write scheme");
                self.blocked.remove(i);
            } else {
                i += 1;
            }
        }

        // Handle new scheme requests
        loop {
            let request = match self.socket.next_request(SignalBehavior::Restart) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    // Scheme likely got unmounted
                    std::process::exit(0);
                }
                Err(err) if err.errno == EAGAIN => break,
                Err(err) => return Err(err.into()),
            };

            match request.kind() {
                RequestKind::Call(call_request) => {
                    if let Some(resp) = call_request.handle_scheme_block(self) {
                        self.socket.write_response(resp, SignalBehavior::Restart)?;
                    } else {
                        self.blocked.push(call_request);
                    }
                }
                RequestKind::SendFd(sendfd_request) => {
                    self.socket.write_response(
                        Response::for_sendfd(&sendfd_request, Err(syscall::Error::new(EOPNOTSUPP))),
                        SignalBehavior::Restart,
                    )?;
                }
                RequestKind::Cancellation(_cancellation_request) => {
                    // FIXME implement cancellation
                }
                RequestKind::MsyncMsg | RequestKind::MunmapMsg | RequestKind::MmapMsg => {
                    unreachable!()
                }
            }
        }

        Ok(())
    }

    // Checks if any conflicting handles already exist
    fn check_locks(&self, disk_i: u32, part_i_opt: Option<u32>) -> Result<()> {
        for (_, handle) in self.handles.iter() {
            match handle {
                Handle::Disk(i) => {
                    if disk_i == *i {
                        return Err(Error::new(ENOLCK));
                    }
                }
                Handle::Partition(i, p) => {
                    if disk_i == *i {
                        match part_i_opt {
                            Some(part_i) => {
                                if part_i == *p {
                                    return Err(Error::new(ENOLCK));
                                }
                            }
                            None => {
                                return Err(Error::new(ENOLCK));
                            }
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
}

impl<T: Disk> SchemeBlock for DiskScheme<T> {
    fn xopen(
        &mut self,
        path_str: &str,
        flags: usize,
        ctx: &CallerCtx,
    ) -> Result<Option<OpenResult>> {
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        let path_str = path_str.trim_matches('/');

        let handle = if path_str.is_empty() {
            if flags & O_DIRECTORY == O_DIRECTORY || flags & O_STAT == O_STAT {
                let mut list = String::new();

                for (nsid, disk) in self.disks.iter() {
                    write!(list, "{}\n", nsid).unwrap();

                    if disk.pt.is_none() {
                        continue;
                    }
                    for part_num in 0..disk.pt.as_ref().unwrap().partitions.len() {
                        write!(list, "{}p{}\n", nsid, part_num).unwrap();
                    }
                }

                Handle::List(list.into_bytes())
            } else {
                return Err(Error::new(EISDIR));
            }
        } else if let Some(p_pos) = path_str.chars().position(|c| c == 'p') {
            let nsid_str = &path_str[..p_pos];

            if p_pos + 1 >= path_str.len() {
                return Err(Error::new(ENOENT));
            }
            let part_num_str = &path_str[p_pos + 1..];

            let nsid = nsid_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;
            let part_num = part_num_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            if let Some(disk) = self.disks.get(&nsid) {
                if disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(ENOENT))?
                    .partitions
                    .get(part_num as usize)
                    .is_some()
                {
                    self.check_locks(nsid, Some(part_num))?;

                    Handle::Partition(nsid, part_num)
                } else {
                    return Err(Error::new(ENOENT));
                }
            } else {
                return Err(Error::new(ENOENT));
            }
        } else {
            let nsid = path_str.parse::<u32>().or(Err(Error::new(ENOENT)))?;

            if self.disks.contains_key(&nsid) {
                self.check_locks(nsid, None)?;
                Handle::Disk(nsid)
            } else {
                return Err(Error::new(ENOENT));
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, handle);
        Ok(Some(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        }))
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<Option<usize>> {
        match *self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref data) => {
                stat.st_mode = MODE_DIR;
                stat.st_size = data.len() as u64;
                Ok(Some(0))
            }
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_blocks = disk.disk().size() / u64::from(disk.block_size());
                stat.st_blksize = disk.block_size();
                stat.st_size = disk.size();
                Ok(Some(0))
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let part = disk
                    .pt
                    .as_ref()
                    .ok_or(Error::new(EBADF))?
                    .partitions
                    .get(part_num as usize)
                    .ok_or(Error::new(EBADF))?;
                stat.st_mode = MODE_FILE;
                stat.st_size = part.size * u64::from(disk.block_size());
                stat.st_blocks = part.size;
                stat.st_blksize = disk.block_size();
                Ok(Some(0))
            }
        }
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<Option<usize>> {
        let handle = self.handles.get(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;

        let scheme_name = self.scheme_name.as_bytes();
        let mut j = 0;
        while i < buf.len() && j < scheme_name.len() {
            buf[i] = scheme_name[j];
            i += 1;
            j += 1;
        }

        if i < buf.len() {
            buf[i] = b':';
            i += 1;
        }

        match *handle {
            Handle::List(_) => (),
            Handle::Disk(number) => {
                let number_str = format!("{}", number);
                let number_bytes = number_str.as_bytes();
                j = 0;
                while i < buf.len() && j < number_bytes.len() {
                    buf[i] = number_bytes[j];
                    i += 1;
                    j += 1;
                }
            }
            Handle::Partition(disk_num, part_num) => {
                let number_str = format!("{}p{}", disk_num, part_num);
                let number_bytes = number_str.as_bytes();
                j = 0;
                while i < buf.len() && j < number_bytes.len() {
                    buf[i] = number_bytes[j];
                    i += 1;
                    j += 1;
                }
            }
        }

        Ok(Some(i))
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(ref handle) => {
                let src = usize::try_from(offset)
                    .ok()
                    .and_then(|o| handle.get(o..))
                    .unwrap_or(&[]);
                let count = core::cmp::min(src.len(), buf.len());
                buf[..count].copy_from_slice(&src[..count]);
                Ok(Some(count))
            }
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.read(None, block, buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.read(Some(part_num as usize), block, buf)
            }
        }
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        offset: u64,
        _fcntl_flags: u32,
    ) -> Result<Option<usize>> {
        match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
            Handle::List(_) => Err(Error::new(EBADF)),
            Handle::Disk(number) => {
                let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.write(None, block, buf)
            }
            Handle::Partition(disk_num, part_num) => {
                let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                let block = offset / u64::from(disk.block_size());
                disk.write(Some(part_num as usize), block, buf)
            }
        }
    }

    fn fsize(&mut self, id: usize) -> Result<Option<u64>> {
        Ok(Some(
            match *self.handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::List(ref handle) => handle.len() as u64,
                Handle::Disk(number) => {
                    let disk = self.disks.get_mut(&number).ok_or(Error::new(EBADF))?;
                    disk.size()
                }
                Handle::Partition(disk_num, part_num) => {
                    let disk = self.disks.get_mut(&disk_num).ok_or(Error::new(EBADF))?;
                    let part = disk
                        .pt
                        .as_ref()
                        .ok_or(Error::new(EBADF))?
                        .partitions
                        .get(part_num as usize)
                        .ok_or(Error::new(EBADF))?;

                    part.size * u64::from(disk.block_size())
                }
            },
        ))
    }

    fn close(&mut self, id: usize) -> Result<Option<usize>> {
        self.handles
            .remove(&id)
            .ok_or(Error::new(EBADF))
            .and(Ok(Some(0)))
    }
}
