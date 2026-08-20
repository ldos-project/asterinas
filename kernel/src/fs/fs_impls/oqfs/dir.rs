// SPDX-License-Identifier: MPL-2.0

//! The volatile directory tree of the OQueue filesystem. Every OQueue appears in the type-keyed
//! OQueue registry, but only those additionally exported to userspace appear in OQFS; they are
//! referred to as "exports" in this file.
//!
//! A single [`DirInode`] type serves every directory in the tree. Its identity is a `prefix`: the
//! sequence of filesystem path components from the OQFS root down to it (the root's prefix is empty).
//! The tree is not cached, because OQueues might get registered or unregistered at runtime.
//! Each `lookup`/`readdir` recomputes children from the live
//! OQueue export registry ([`registry::list_export_paths`]), so queues that register or unregister
//! at runtime appear and disappear immediately. And this will be fast, since registry used to hold
//! the OQueues exposed to the userspace is implemented with BTree.
//!
//! A directory plays one of two roles:
//!
//! - **OQueue leaf** — It lists [`metadata::FILE_NAME`] plus [`strong_observe::FILE_NAME`] and/or
//!   [`produce::FILE_NAME`], depending on which attachments the export currently carries (see
//!   [`leaf_file_names`]); an export may carry either, or both, independently.
//! - **non-leaf** (including the root) — It lists the distinct next components of every exported
//!   path that it is a prefix of.
//!
//! A path that is simultaneously an exported queue and a prefix of a longer exported path is a
//! namespace collision: it is served as a leaf, the deeper queues are hidden, and the collision is
//! logged (rather than being silently ignored).

use core::time::Duration;

use inherit_methods_macro::inherit_methods;
use ostd::orpc::{
    oqueue::registry,
    path::{Path, PathComponentRef},
};

use super::{BLOCK_SIZE, Common, OQUEUE_ROOT_INO, OQueueFs, metadata, produce, strong_observe};
use crate::{
    fs::{
        file::{InodeMode, InodeType, StatusFlags, mkmod},
        utils::DirentVisitor,
        vfs::{
            file_system::{FileSystem, SuperBlock},
            inode::{
                Extension, FileOps, Inode, Metadata, MknodType, RenameMode, RevalidationPolicy,
            },
            path::{is_dot, is_dotdot},
        },
    },
    prelude::*,
    process::{Gid, Uid},
};

/// Returns the paths of the OQueues that are currently exported and still alive.
///
/// Dead exports (whose OQueue has been dropped) are evicted before the registry content is returned.
fn live_export_paths() -> impl Iterator<Item = Path> {
    registry::clean_exports();
    registry::list_export_paths().into_iter()
}

/// Returns the fixed files served by an OQueue leaf directory: [`metadata::FILE_NAME`] plus
/// [`strong_observe::FILE_NAME`] and/or [`produce::FILE_NAME`], depending on which attachments the
/// export currently carries. Only the metadata file is listed if the export has disappeared.
fn leaf_file_names(oqueue: &Path) -> Vec<&'static str> {
    let Some(export) = registry::lookup_export(oqueue) else {
        return vec![metadata::FILE_NAME];
    };

    let mut names = Vec::new();
    if export.supports_observe() {
        names.push(strong_observe::FILE_NAME);
    }
    if export.supports_produce() {
        names.push(produce::FILE_NAME);
    }
    names.push(metadata::FILE_NAME);
    names
}

/// convert an OQueue [`Path`] to filesystem path components: names stay as-is, indices become their
/// decimal string. For example `raid1.io[0]` maps to `["raid1", "io", "0"]`.
fn path_to_segments(path: &Path) -> Vec<String> {
    path.components()
        .iter()
        .map(|component| match component.borrow() {
            PathComponentRef::Name(name) => name.to_string(),
            PathComponentRef::Index(index) => index.to_string(),
        })
        .collect()
}

/// A directory in the OQueue filesystem.
pub(super) struct DirInode {
    /// The filesystem path components from the root to this directory (empty for the root).
    prefix: Vec<String>,
    this: Weak<DirInode>, // for .
    fs: Weak<OQueueFs>,
    common: Common,
}

impl DirInode {
    /// Creates the root directory inode.
    pub(super) fn new_root(fs: Weak<OQueueFs>, sb: &SuperBlock) -> Arc<dyn Inode> {
        Self::new_inode(fs, Vec::new(), OQUEUE_ROOT_INO, sb.container_dev_id)
    }

    /// Creates a non-root directory inode for the given filesystem `prefix`.
    fn new_dir(fs: Weak<OQueueFs>, prefix: Vec<String>) -> Arc<dyn Inode> {
        let strong_fs = fs.upgrade().unwrap();
        let ino = strong_fs.alloc_id();
        let container_dev_id = strong_fs.sb().container_dev_id;
        Self::new_inode(fs, prefix, ino, container_dev_id)
    }

    fn new_inode(
        fs: Weak<OQueueFs>,
        prefix: Vec<String>,
        ino: u64,
        container_dev_id: device_id::DeviceId,
    ) -> Arc<dyn Inode> {
        // Root-only, like every other OQFS inode. `Metadata::new_dir` always sets the owner to
        // root, so owner-only `rx` lets root list and traverse while denying everyone else. This
        // has to match the leaf files: leaving directories world-traversable would let an
        // unprivileged process walk `/oqueues` and enumerate every exported OQueue and its path,
        // even though it could not open `strong_observe` or `produce`.
        let metadata = Metadata::new_dir(ino, mkmod!(u+rx), BLOCK_SIZE, container_dev_id);
        let fs_weak: Weak<dyn FileSystem> = fs.clone();
        Arc::new_cyclic(|weak_self| DirInode {
            prefix,
            this: weak_self.clone(),
            fs,
            common: Common::new(metadata, fs_weak),
        })
    }

    fn this(&self) -> Arc<dyn Inode> {
        self.this.upgrade().unwrap()
    }

    /// Returns the parent directory inode.
    fn parent_inode(&self) -> Arc<dyn Inode> {
        match self.prefix.split_last() {
            Some((_last, parent_prefix)) => {
                DirInode::new_dir(self.fs.clone(), parent_prefix.to_vec())
            }
            None => self.this(),
        }
    }

    /// Returns the exported OQueue path if this directory is a leaf (its `prefix` matches an
    /// exported queue exactly), or `None` otherwise.
    fn as_oqueue(&self) -> Option<Path> {
        live_export_paths().find(|path| path_to_segments(path) == self.prefix)
    }

    /// Logs the namespace collision where this leaf directory is *also* a prefix of deeper exported
    /// paths, so those deeper queues are hidden behind the leaf's observation files.
    fn warn_if_prefix_conflict(&self, oqueue: &Path) {
        if !self.child_dir_names().is_empty() {
            error!(
                "OQueue export \"{}\" is also a prefix of deeper exports; serving it as a leaf \
                 and hiding the deeper queues",
                oqueue
            );
        }
    }

    /// Returns the sorted, distinct next path components under the current directory.
    ///
    /// Multiple exported paths can share the same next component under this prefix (e.g. both
    /// `raid1.io[0]` and `raid1.io[1]` yield `io` under `raid1`), so the collected components are
    /// deduplicated.
    fn child_dir_names(&self) -> Vec<String> {
        let mut names: Vec<String> = live_export_paths()
            .filter_map(|path| {
                let segments = path_to_segments(&path);
                (segments.len() > self.prefix.len() && segments.starts_with(&self.prefix))
                    .then(|| segments[self.prefix.len()].clone())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Returns whether `name` is currently a valid child of this directory.
    fn has_child(&self, name: &str) -> bool {
        if let Some(oqueue) = self.as_oqueue() {
            leaf_file_names(&oqueue).contains(&name)
        } else {
            self.child_dir_names().iter().any(|child| child == name)
        }
    }
}

impl FileOps for DirInode {
    /// You can't read a dir
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }

    /// You can't write a dir.
    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let this = self.this();
        let parent = self.parent_inode();

        // Entries beyond `.` and `..` (reserved offsets 1 and 2), as `(name, type)`.
        let children: Vec<(String, InodeType)> = if let Some(oqueue) = self.as_oqueue() {
            self.warn_if_prefix_conflict(&oqueue);
            leaf_file_names(&oqueue)
                .into_iter()
                .map(|name| (name.to_string(), InodeType::File))
                .collect()
        } else {
            self.child_dir_names()
                .into_iter()
                .map(|name| (name, InodeType::Dir))
                .collect()
        };

        let try_readdir = |cursor: &mut usize, visitor: &mut dyn DirentVisitor| -> Result<()> {
            for (name, inode, next_offset) in [(".", &this, 1usize), ("..", &parent, 2usize)] {
                if next_offset > *cursor {
                    visitor.visit(name, inode.ino(), InodeType::Dir, next_offset)?;
                    *cursor = next_offset;
                }
            }
            for (index, (name, type_)) in children.iter().enumerate() {
                let next_offset = 3 + index;
                if next_offset > *cursor {
                    // A placeholder inode number: real inodes are created on demand during lookup.
                    visitor.visit(name, this.ino(), *type_, next_offset)?;
                    *cursor = next_offset;
                }
            }
            Ok(())
        };

        let mut cursor = offset;
        match try_readdir(&mut cursor, visitor) {
            Err(e) if cursor == offset => Err(e),
            _ => Ok(cursor - offset),
        }
    }
}

#[inherit_methods(from = "self.common")]
impl Inode for DirInode {
    fn size(&self) -> usize;
    fn metadata(&self) -> Result<Metadata>;
    fn extension(&self) -> &Extension;
    fn ino(&self) -> u64;
    fn mode(&self) -> Result<InodeMode>;
    fn set_mode(&self, mode: InodeMode) -> Result<()>;
    fn owner(&self) -> Result<Uid>;
    fn set_owner(&self, uid: Uid) -> Result<()>;
    fn group(&self) -> Result<Gid>;
    fn set_group(&self, gid: Gid) -> Result<()>;
    fn atime(&self) -> Duration;
    fn set_atime(&self, time: Duration);
    fn mtime(&self) -> Duration;
    fn set_mtime(&self, time: Duration);
    fn ctime(&self) -> Duration;
    fn set_ctime(&self, time: Duration);
    fn fs(&self) -> Arc<dyn FileSystem>;

    fn resize(&self, _new_size: usize) -> Result<()> {
        Err(Error::new(Errno::EISDIR))
    }

    fn type_(&self) -> InodeType {
        InodeType::Dir
    }

    fn create(&self, _name: &str, _type_: InodeType, _mode: InodeMode) -> Result<Arc<dyn Inode>> {
        Err(Error::new(Errno::EPERM))
    }

    fn mknod(&self, _name: &str, _mode: InodeMode, _type_: MknodType) -> Result<Arc<dyn Inode>> {
        Err(Error::new(Errno::EPERM))
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        if is_dot(name) {
            return Ok(self.this());
        }
        if is_dotdot(name) {
            return Ok(self.parent_inode());
        }

        if let Some(oqueue) = self.as_oqueue() {
            // This directory is an exported OQueue, so its children are the fixed files for its
            // single direction (see `leaf_file_names`).
            self.warn_if_prefix_conflict(&oqueue);
            if !leaf_file_names(&oqueue).contains(&name) {
                return Err(Error::new(Errno::ENOENT));
            }
            return match name {
                strong_observe::FILE_NAME => Ok(strong_observe::new_inode(self.fs.clone(), oqueue)),
                produce::FILE_NAME => Ok(produce::new_inode(self.fs.clone(), oqueue)),
                metadata::FILE_NAME => Ok(metadata::new_inode(self.fs.clone(), oqueue)),
                _ => Err(Error::new(Errno::ENOENT)),
            };
        }

        // This is a namespace directory; the child is a subdirectory if the extended prefix is a
        // prefix of some exported path.
        if !self.has_child(name) {
            return_errno_with_message!(Errno::ENOENT, "the file does not exist");
        }
        let mut child_prefix = self.prefix.clone();
        child_prefix.push(name.to_string());
        Ok(DirInode::new_dir(self.fs.clone(), child_prefix))
    }

    fn link(&self, _old: &Arc<dyn Inode>, _name: &str) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn rmdir(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn rename(
        &self,
        _old_name: &str,
        _target: &Arc<dyn Inode>,
        _new_name: &str,
        _mode: RenameMode,
    ) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    // The namespace is volatile, so cached lookups must always be revalidated against the live
    // registry: an entry may vanish (its queue unregistered) or appear (a new queue registered).
    fn revalidation_policy(&self) -> RevalidationPolicy {
        RevalidationPolicy::REVALIDATE_EXISTS | RevalidationPolicy::REVALIDATE_ABSENT
    }

    fn revalidate_exists(&self, name: &str, _child: &dyn Inode) -> bool {
        self.has_child(name)
    }

    fn revalidate_absent(&self, name: &str) -> bool {
        !self.has_child(name)
    }

    fn seek_end(&self) -> Option<usize> {
        Some(0)
    }
}
