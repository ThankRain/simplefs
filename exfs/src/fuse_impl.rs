use std::cmp::min;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::str::FromStr;

use fuse::{
    FileAttr, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request,
};
use libc::{EEXIST, ENOENT, O_RDWR};
use log::debug;
use time::Timespec;

use crate::cache::file_handler::FileHandler;
use crate::layout::data_block::{DirEntry, FileName};
use crate::layout::inode::{DIR, FILE, FileType, Inode, SYMBOL};
use crate::manager::block_cache_manager::{BlockCacheDevice, trim_zero};
use crate::utils::slice::vec2slice;

impl Filesystem for BlockCacheDevice {
    fn lookup(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEntry) {
        // debug!("parent:{},req:{:?},name:{:?}", _parent, _req, _name);
        let ttl = Timespec::new(60, 0);
        let r = reply;
        match self.lookup_inter(ino_id(_parent), name2(_name)) {
            Ok(entry) => {
                let inode = self.inode(entry.inode as usize);
                r.entry(&ttl, &file_attr(inode, id_ino(entry.inode as usize)), 0);
            }
            Err(e) => {
                debug!("Lookup error: {:?}", e);
                r.error(e);
            }
        }
    }

    fn forget(&mut self, _req: &Request, _ino: u64, _nlookup: u64) {
        // debug!("Forget: {}", _ino)
    }
    fn getattr(&mut self, _req: &Request, _ino: u64, reply: ReplyAttr) {
        let ttl = Timespec::new(60, 0);
        let inode_id = ino_id(_ino);
        let inode = self.inode(inode_id);
        let attr = file_attr(inode, _ino);
        reply.attr(&ttl, &attr)
    }

    fn setattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<Timespec>,
        _mtime: Option<Timespec>,
        _fh: Option<u64>,
        _crtime: Option<Timespec>,
        _chgtime: Option<Timespec>,
        _bkuptime: Option<Timespec>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let inode = self.modify_inode(ino_id(_ino), |ino| {
            if let Some(v) = _mode {
                ino.mode = v as u16
            }
            if let Some(v) = _uid {
                ino.uid = v
            }
            if let Some(v) = _gid {
                ino.gid = v
            }
            if let Some(v) = _size {
                ino.size = v
            }
            // if let Some(_) = _atime {
            //     // ino. = v
            // }
            if let Some(v) = _mtime {
                ino.modified = v.sec as u64
            }
            if let Some(v) = _crtime {
                ino.created = v.sec as u64
            }
            ino.clone()
        });
        let ttl = Timespec::new(60, 0);
        reply.attr(&ttl, &file_attr(inode, _ino))
    }

    fn readlink(&mut self, _req: &Request, _ino: u64, reply: ReplyData) {
        // debug!("ReadLink: {}", _ino)
        let buf = self.read_all(ino_id(_ino));
        reply.data(buf.as_ref())
    }

    fn mknod(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let file = self.mk_file(
            _name.to_str().unwrap(),
            ino_id(_parent),
            FILE << 12 | _mode as u16,
        );
        let ttl = Timespec::new(60, 0);
        match file {
            Ok(v) => {
                let inode = self.inode(v);
                let attr = file_attr(inode, id_ino(v));
                debug!("Mknod: v:{}, {:#?}", v, attr);
                reply.entry(&ttl, &attr, 0)
            }
            Err(e) => {
                debug!("Mknod error: {:?}", e);
                reply.error(e)
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        reply: ReplyEntry,
    ) {
        let folder = self.mk_file(
            _name.to_str().unwrap(),
            ino_id(_parent),
            DIR << 12 | _mode as u16,
        );
        let ttl = Timespec::new(60, 0);
        match folder {
            Ok(v) => {
                let inode = self.inode(v);
                let attr = file_attr(inode, id_ino(v));
                // debug!("Mkdir: v:{}, {:#?}", v, attr);
                reply.entry(&ttl, &attr, 0)
            }
            Err(e) => {
                debug!("Mkdir error: {:?}", e);
                reply.error(e)
            }
        }
    }

    fn unlink(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        let parent_id = ino_id(_parent);
        match self.rm(parent_id, name2(_name), false) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn rmdir(&mut self, _req: &Request, _parent: u64, _name: &OsStr, _reply: ReplyEmpty) {
        // debug!("RmDir: {:?}", _name)
    }

    fn symlink(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _link: &Path,
        reply: ReplyEntry,
    ) {
        // debug!("SymLink: {:?}", _name)
        let symbol = self.mk_file(
            _name.to_str().unwrap(),
            ino_id(_parent),
            SYMBOL << 12 | 0o744u16,
        );
        let ttl = Timespec::new(60, 0);
        match symbol {
            Ok(v) => {
                let buf = _link.to_str().unwrap();
                let inode = self.inode(v);
                if let Err(e) = self.write_all(0, v, buf.as_ref(), true) {
                    reply.error(e);
                } else {
                    let attr = file_attr(inode, id_ino(v));
                    reply.entry(&ttl, &attr, 0);
                }
            }
            Err(e) => {
                debug!("Symbol link error: {:?}", e);
                reply.error(e)
            }
        }
    }

    fn rename(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEmpty,
    ) {
        let parent = ino_id(_parent);
        let new_parent = ino_id(_newparent);
        match self.rename_inner(parent, name2(_name), new_parent, name2(_newname)) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn link(
        &mut self,
        _req: &Request,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        let inode_id = ino_id(_ino);
        let parent_id = ino_id(_newparent);
        let mut dirs = self.dir_list(parent_id);
        match self.lookup_inter(parent_id, name2(_newname)) {
            Err(ENOENT) => {
                dirs.push(DirEntry {
                    name: name2(_newname),
                    inode: inode_id as u64,
                });
                let buf: Vec<u8> = vec2slice(dirs);
                if let Err(e) = self.write_all(0, parent_id, &buf, true) {
                    debug!("link:235 error: {}", e);
                    reply.error(e);
                    return;
                }
                let inode = self.inode(inode_id);
                let ttl = Timespec::new(60, 0);
                reply.entry(&ttl, &file_attr(inode, _ino), 0)
            }
            Ok(_) => {
                reply.error(EEXIST)
            }
            Err(e) => {
                reply.error(e);
            }
        }
    }

    fn open(&mut self, _req: &Request, _ino: u64, _flags: u32, reply: ReplyOpen) {
        let fh = self.open_inner(ino_id(_ino), 0, _flags as u16);
        reply.opened(fh, _flags)
    }

    fn read(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _size: u32,
        reply: ReplyData,
    ) {
        let mut buf: Vec<u8> = Vec::new();
        for _ in 0.._size {
            buf.push(0)
        }
        FileHandler {
            inode: ino_id(_ino),
            offset: _offset as usize,
            flags: 0,
        }
            .read(self, &mut buf);
        debug!("Read {}: 【{:?}】", _ino, trim_zero(buf.clone()));
        reply.data(&buf)
    }

    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: &[u8],
        _flags: u32,
        reply: ReplyWrite,
    ) {
        match self.write_inner(_offset as usize, ino_id(_ino), _data) {
            Ok(_) => reply.written(_data.len() as u32),
            Err(e) => reply.error(e),
        }
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        self.sync();
        reply.ok()
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        match self.close_inner(_fh) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn opendir(&mut self, _req: &Request, _ino: u64, _flags: u32, reply: ReplyOpen) {
        // debug!("OpenDir: {}", _ino);
        reply.opened(_ino, O_RDWR as u32);
    }

    fn readdir(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        reply: ReplyDirectory,
    ) {
        let mut r = reply;
        if _offset != 0 {
            r.ok();
            return;
        }
        match self.ls_(ino_id(_ino)) {
            Ok(v) => {
                r.add(
                    _ino,
                    1,
                    fuse::FileType::Directory,
                    OsString::from_str("..").unwrap(),
                );
                for entry in v {
                    debug!("DirEntry: {:?} ({})", name(entry.name), entry.inode);
                    let inode = self.inode(entry.inode as usize);
                    r.add(
                        entry.inode,
                        1,
                        file_type(inode.file_type()),
                        name(entry.name).as_os_str(),
                    );
                }
            }
            Err(e) => {
                debug!("ReadDir error: {:?}", e)
            }
        }
        r.ok()
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, _reply: ReplyStatfs) {
        // debug!("StatsFS: {}", _ino)
    }

    fn getxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        // debug!("GetXAttr: {}", _ino);
        reply.size(0)
    }

    fn access(&mut self, _req: &Request, _ino: u64, _mask: u32, reply: ReplyEmpty) {
        // debug!("Access: {}", _ino);
        reply.ok()
    }

    fn create(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _flags: u32,
        reply: ReplyCreate,
    ) {
        let file = self.mk_file(
            _name.to_str().unwrap(),
            ino_id(_parent),
            FILE << 12 | _mode as u16,
        );
        let ttl = Timespec::new(60, 0);
        match file {
            Ok(v) => {
                let inode = self.inode(v);
                let attr = file_attr(inode, id_ino(v));
                let fh = self.open_inner(v, 0, _flags as u16);
                // debug!("Create: v:{}, {:#?}", v, attr);
                reply.created(&ttl, &attr, 0, fh, _flags);
            }
            Err(e) => {
                debug!("Create error: {:?}", e);
                reply.error(e)
            }
        }
    }
}

fn file_type(typ: FileType) -> fuse::FileType {
    match typ {
        FileType::Socket => fuse::FileType::Socket,
        FileType::SymbolLink => fuse::FileType::Symlink,
        FileType::File => fuse::FileType::RegularFile,
        FileType::BlockDevice => fuse::FileType::BlockDevice,
        FileType::Dir => fuse::FileType::Directory,
        FileType::CharDevice => fuse::FileType::CharDevice,
        FileType::FIFO => fuse::FileType::NamedPipe,
        FileType::UNK => fuse::FileType::RegularFile,
    }
}

fn name(name: FileName) -> OsString {
    OsString::from_vec(trim_zero(name.to_vec()))
}

fn name2(name: &OsStr) -> FileName {
    let mut file_name = [0u8; 56];
    let name_str = name.to_str().unwrap();
    let len = min(name_str.len(), 56);
    file_name[..len].copy_from_slice(name_str[..len].as_bytes());
    file_name
}

fn file_attr(inode: Inode, _ino: u64) -> FileAttr {
    let mode = inode.mode & ((1 << 9) - 1);
    // debug!(
    //     "FMode: {:b},{:o}, Type: {:?}",
    //     inode.mode,
    //     mode,
    //     inode.file_type()
    // );
    FileAttr {
        ino: _ino,
        size: inode.size,
        blocks: inode.blocks(),
        atime: Timespec::new(inode.modified as i64, 0),
        mtime: Timespec::new(inode.modified as i64, 0),
        ctime: Timespec::new(inode.created as i64, 0),
        crtime: Timespec::new(inode.created as i64, 0),
        perm: mode,
        kind: file_type(inode.file_type()),
        nlink: inode.link_count,
        uid: inode.uid,
        gid: inode.gid,
        rdev: 0,
        flags: 0,
    }
}

fn ino_id(ino_: u64) -> usize {
    (ino_) as usize
}

fn id_ino(inode_id: usize) -> u64 {
    (inode_id) as u64
}
