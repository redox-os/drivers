use redox_scheme::scheme::SchemeSync;
use redox_scheme::{CallerCtx, OpenResult};
use std::convert::TryFrom;
use syscall::dirent::{DirEntry, DirentBuf, DirentKind};
use syscall::schemev2::NewFdFlags;
use syscall::{
    Error, Result, EACCES, EINVAL, EMFILE, ENOENT, ENOTDIR, MODE_DIR, MODE_FILE, O_WRONLY,
};

pub struct Ps2Scheme {
    pub keymap: String,
    pub keymap_list: String,
}

impl Ps2Scheme {
    pub fn new(keymap: String, keymap_list: Vec<&str>) -> Ps2Scheme {
        let scheme = Ps2Scheme {
            keymap,
            keymap_list: keymap_list.join("\n"),
        };
        scheme
    }
}

impl SchemeSync for Ps2Scheme {
    fn open(&mut self, path_str: &str, flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        let path = path_str.trim_start_matches('/');
        if flags & O_WRONLY != 0 {
            if ctx.uid != 0 || ctx.gid != 0 {
                return Err(Error::new(EACCES));
            } else if path != "keymap" {
                return Err(Error::new(EINVAL));
            }
        }

        match path {
            "" => Ok(OpenResult::ThisScheme {
                number: 0,
                flags: NewFdFlags::empty(),
            }),
            "keymap" => Ok(OpenResult::ThisScheme {
                number: 1,
                flags: NewFdFlags::POSITIONED,
            }),
            "keymap_list" => Ok(OpenResult::ThisScheme {
                number: 2,
                flags: NewFdFlags::POSITIONED,
            }),
            _ => Err(Error::new(ENOENT)),
        }
    }
    fn getdents<'buf>(
        &mut self,
        id: usize,
        mut buf: DirentBuf<&'buf mut [u8]>,
        opaque_offset: u64,
    ) -> Result<DirentBuf<&'buf mut [u8]>> {
        if id != 0 {
            return Err(Error::new(ENOTDIR));
        }
        let Ok(offset) = usize::try_from(opaque_offset) else {
            return Ok(buf);
        };
        for (this_idx, name) in ["keymap", "keymap_list"].iter().enumerate().skip(offset) {
            buf.entry(DirEntry {
                inode: this_idx as u64,
                next_opaque_id: this_idx as u64 + 1,
                kind: DirentKind::Regular,
                name,
            })?;
        }
        Ok(buf)
    }

    fn fstat(&mut self, id: usize, stat: &mut syscall::Stat, _ctx: &CallerCtx) -> Result<()> {
        stat.st_size = 0;
        stat.st_mode = match id {
            0 => 0o555 | MODE_DIR,
            1 => 0o644 | MODE_FILE,
            2 => 0o444 | MODE_FILE,
            _ => return Err(Error::new(ENOENT)),
        };
        Ok(())
    }

    fn fpath(&mut self, _id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        let path = b"/scheme/ps2";

        let mut i = 0;
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }

        Ok(i)
    }

    fn fsync(&mut self, _id: usize, _ctx: &CallerCtx) -> Result<()> {
        Ok(())
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        if offset != 0 {
            return Ok(0);
        }
        let value = match id {
            1 => self.keymap.as_bytes(),
            2 => self.keymap_list.as_bytes(),
            _ => {
                return Err(Error::new(ENOENT));
            }
        };

        if buf.len() + 2 < value.len() {
            return Err(Error::new(EMFILE));
        }
        buf[..value.len()].copy_from_slice(value);
        buf[value.len()] = b'\n';
        buf[value.len() + 1] = b'\0';
        Ok(value.len() + 2)
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        if offset != 0 || id != 1 {
            return Ok(0);
        }
        let new_keymap = String::from_utf8(buf.to_vec()).map_err(|_| Error::new(EINVAL))?;
        self.keymap = new_keymap.trim().to_string();
        Ok(buf.len())
    }
}
