// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::BTreeSet, string::String, sync::Arc, vec, vec::Vec};
use core::time::Duration;

use mariposa_oqueue_procfs::StreamSource;

use super::{
    BLOCK_SIZE, ProcFS,
    template::{DirOps, ProcDirBuilder},
};
use crate::{
    events::IoEvents,
    fs::utils::{
        DirEntryVecExt, FileSystem, Inode, InodeMode, InodeType, IoctlCmd, Metadata, MknodType,
    },
    prelude::*,
    process::{Gid, Uid, signal::PollHandle},
};

const OQUEUES_ROOT: &str = "/proc/oqueues";
const STREAM_READ_CHUNK: usize = 64 * 1024;

pub struct OqueuesDirOps {
    prefix: Vec<String>,
}

impl OqueuesDirOps {
    pub fn new_root(parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        Self::new_inode(Vec::new(), parent)
    }

    fn new_inode(prefix: Vec<String>, parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        ProcDirBuilder::new(Self { prefix })
            .parent(parent)
            .volatile()
            .build()
            .unwrap()
    }

    fn child_prefix(&self, name: &str) -> Vec<String> {
        self.prefix
            .iter()
            .cloned()
            .chain(core::iter::once(String::from(name)))
            .collect()
    }

    fn proc_path(prefix: &[String]) -> String {
        let mut path = String::from(OQUEUES_ROOT);
        for component in prefix {
            path.push('/');
            path.push_str(component);
        }
        path
    }

    fn registered_components(path: &str) -> Option<Vec<&str>> {
        let suffix = path.strip_prefix(OQUEUES_ROOT)?;
        if suffix.is_empty() {
            return Some(Vec::new());
        }

        Some(suffix.strip_prefix('/')?.split('/').collect())
    }

    fn child_names(&self) -> BTreeSet<String> {
        let mut children = BTreeSet::new();

        for path in mariposa_oqueue_procfs::paths() {
            let Some(components) = Self::registered_components(&path) else {
                continue;
            };

            if components.len() <= self.prefix.len() {
                continue;
            }

            if components
                .iter()
                .zip(&self.prefix)
                .all(|(component, prefix)| component == prefix)
            {
                children.insert(String::from(components[self.prefix.len()]));
            }
        }

        children
    }

    fn child_inode(&self, this_ptr: Weak<dyn Inode>, name: &str) -> Result<Arc<dyn Inode>> {
        let child_prefix = self.child_prefix(name);
        let proc_path = Self::proc_path(&child_prefix);

        if let Ok(source) = mariposa_oqueue_procfs::open_stream(&proc_path) {
            return Ok(OqueueStreamInode::new(source, this_ptr));
        }

        if mariposa_oqueue_procfs::has_registered_prefix(&proc_path) {
            return Ok(Self::new_inode(child_prefix, this_ptr));
        }

        return_errno!(Errno::ENOENT);
    }
}

impl DirOps for OqueuesDirOps {
    fn lookup_child(&self, this_ptr: Weak<dyn Inode>, name: &str) -> Result<Arc<dyn Inode>> {
        self.child_inode(this_ptr, name)
    }

    fn populate_children(&self, this_ptr: Weak<dyn Inode>) {
        let this = this_ptr.upgrade().unwrap();
        let this_dir = this
            .downcast_ref::<super::template::ProcDir<Self>>()
            .unwrap();
        let mut cached_children = this_dir.cached_children().write();

        for name in self.child_names() {
            if let Ok(inode) = self.child_inode(this_ptr.clone(), &name) {
                cached_children.put_entry_if_not_found(&name, || inode.clone());
            }
        }
    }
}

struct OqueueStreamInode {
    source: Arc<dyn StreamSource>,
    metadata: RwLock<Metadata>,
    fs: Weak<dyn FileSystem>,
}

impl OqueueStreamInode {
    fn new(source: Arc<dyn StreamSource>, parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        let fs = parent.upgrade().unwrap().fs();
        let ino = fs.downcast_ref::<ProcFS>().unwrap().alloc_id();
        Arc::new(Self {
            source,
            metadata: RwLock::new(Metadata::new_file(
                ino,
                InodeMode::from_bits_truncate(0o444),
                BLOCK_SIZE,
            )),
            fs: Arc::downgrade(&fs),
        })
    }
}

impl Inode for OqueueStreamInode {
    fn size(&self) -> usize {
        self.metadata.read().size
    }

    fn metadata(&self) -> Metadata {
        *self.metadata.read()
    }

    fn ino(&self) -> u64 {
        self.metadata.read().ino
    }

    fn mode(&self) -> Result<InodeMode> {
        Ok(self.metadata.read().mode)
    }

    fn set_mode(&self, mode: InodeMode) -> Result<()> {
        self.metadata.write().mode = mode;
        Ok(())
    }

    fn owner(&self) -> Result<Uid> {
        Ok(self.metadata.read().uid)
    }

    fn set_owner(&self, uid: Uid) -> Result<()> {
        self.metadata.write().uid = uid;
        Ok(())
    }

    fn group(&self) -> Result<Gid> {
        Ok(self.metadata.read().gid)
    }

    fn set_group(&self, gid: Gid) -> Result<()> {
        self.metadata.write().gid = gid;
        Ok(())
    }

    fn atime(&self) -> Duration {
        self.metadata.read().atime
    }

    fn set_atime(&self, time: Duration) {
        self.metadata.write().atime = time;
    }

    fn mtime(&self) -> Duration {
        self.metadata.read().mtime
    }

    fn set_mtime(&self, time: Duration) {
        self.metadata.write().mtime = time;
    }

    fn ctime(&self) -> Duration {
        self.metadata.read().ctime
    }

    fn set_ctime(&self, time: Duration) {
        self.metadata.write().ctime = time;
    }

    fn resize(&self, _new_size: usize) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn type_(&self) -> InodeType {
        InodeType::File
    }

    fn read_at(&self, _offset: usize, writer: &mut VmWriter) -> Result<usize> {
        if writer.avail() == 0 {
            return Ok(0);
        }

        let mut buf = vec![0u8; writer.avail().min(STREAM_READ_CHUNK)];
        loop {
            let nread = self.source.poll_into(&mut buf);
            if nread > 0 {
                writer.write_fallible(&mut buf[..nread].into())?;
                return Ok(nread);
            }
            self.source.wait_readable();
        }
    }

    fn read_direct_at(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        self.read_at(offset, writer)
    }

    fn write_at(&self, _offset: usize, _reader: &mut VmReader) -> Result<usize> {
        Err(Error::new(Errno::EPERM))
    }

    fn write_direct_at(&self, _offset: usize, _reader: &mut VmReader) -> Result<usize> {
        Err(Error::new(Errno::EPERM))
    }

    fn read_link(&self) -> Result<String> {
        Err(Error::new(Errno::EINVAL))
    }

    fn write_link(&self, _target: &str) -> Result<()> {
        Err(Error::new(Errno::EINVAL))
    }

    fn ioctl(&self, _cmd: IoctlCmd, _arg: usize) -> Result<i32> {
        Err(Error::new(Errno::EPERM))
    }

    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        if self.source.is_readable() {
            IoEvents::IN & mask
        } else {
            IoEvents::empty()
        }
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.upgrade().unwrap()
    }

    fn is_dentry_cacheable(&self) -> bool {
        false
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn create(&self, _name: &str, _type_: InodeType, _mode: InodeMode) -> Result<Arc<dyn Inode>> {
        Err(Error::new(Errno::ENOTDIR))
    }

    fn mknod(&self, _name: &str, _mode: InodeMode, _type_: MknodType) -> Result<Arc<dyn Inode>> {
        Err(Error::new(Errno::ENOTDIR))
    }
}
