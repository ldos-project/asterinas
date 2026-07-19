// SPDX-License-Identifier: MPL-2.0

//! The OQueue filesystem (`/oqueues`).
//!
//! This pseudo-filesystem exposes OQueues that have been exported to userspace (see
//! [`ostd::orpc::oqueue::registry`]) as a procfs-like tree, so a userspace program can read the
//! in-kernel statistics flowing through them.
//!
//! # Layout
//!
//! The directory tree mirrors the OQueue export registry and is recomputed live on every
//! `lookup`/`readdir`, since queues register and unregister at runtime. An OQueue whose path is
//! `a.b[3].c` appears as the directory `/oqueues/a/b/3/c` (names become directory components,
//! indices become numeric components). Each such leaf directory contains:
//!
//! - `strong_observe` — a per-open stream of every value produced after `open`, as a
//!   CBOR byte stream (see [`strong_observe`]). Behaves like a char device.
//! - `metadata.yaml` — a human-readable description of the queue (see [`metadata`]).
//!
//! # Modules
//!
//! - [`dir`]: the volatile directory tree.
//! - [`strong_observe`]: the `strong_observe` stream file.
//! - [`metadata`]: the `metadata.yaml` file.

use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use crate::{
    fs::{
        file::InodeMode,
        pseudofs::AnonDeviceId,
        utils::NAME_MAX,
        vfs::{
            file_system::{FileSystem, FsEventSubscriberStats, SuperBlock},
            inode::{Extension, Inode, Metadata},
            registry::{FsCreationCtx, FsProperties, FsType},
        },
    },
    prelude::*,
    process::{Gid, Uid},
};

mod dir;
mod metadata;
mod strong_observe;

/// Magic number for the OQueue filesystem (`"oqfs"`).
const OQUEUE_MAGIC: u64 = 0x6f71_6673;
/// Root inode ID.
const OQUEUE_ROOT_INO: u64 = 1;
/// Block size.
const BLOCK_SIZE: usize = 1024;

/// Registers the OQueue filesystem type so it can be mounted.
pub(super) fn init() {
    crate::fs::vfs::registry::register(&OQueueFsType).unwrap();
}

struct OQueueFs {
    _anon_device_id: AnonDeviceId,
    sb: SuperBlock,
    root: Arc<dyn Inode>,
    /// Allocates inode IDs for the inodes created on demand during path walks.
    inode_allocator: AtomicU64,
    fs_event_subscriber_stats: FsEventSubscriberStats,
}

impl OQueueFs {
    fn new() -> Arc<Self> {
        let anon_device_id =
            AnonDeviceId::acquire().expect("no device ID is available for oqueuefs");
        let sb = SuperBlock::new(OQUEUE_MAGIC, BLOCK_SIZE, NAME_MAX, anon_device_id.id());
        Arc::new_cyclic(|weak_fs| Self {
            _anon_device_id: anon_device_id,
            sb: sb.clone(),
            root: dir::DirInode::new_root(weak_fs.clone(), &sb),
            inode_allocator: AtomicU64::new(OQUEUE_ROOT_INO + 1),
            fs_event_subscriber_stats: FsEventSubscriberStats::new(),
        })
    }

    /// Allocates a fresh inode ID.
    fn alloc_id(&self) -> u64 {
        self.inode_allocator.fetch_add(1, Ordering::Relaxed)
    }
}

impl FileSystem for OQueueFs {
    fn name(&self) -> &'static str {
        "oqfs"
    }

    fn sync(&self) -> Result<()> {
        Ok(())
    }

    fn root_inode(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn sb(&self) -> SuperBlock {
        self.sb.clone()
    }

    fn fs_event_subscriber_stats(&self) -> &FsEventSubscriberStats {
        &self.fs_event_subscriber_stats
    }
}

struct OQueueFsType;

impl FsType for OQueueFsType {
    fn name(&self) -> &'static str {
        "oqueue"
    }

    fn properties(&self) -> FsProperties {
        FsProperties::empty()
    }

    fn create(&self, _fs_creation_ctx: &FsCreationCtx) -> Result<Arc<dyn FileSystem>> {
        Ok(OQueueFs::new())
    }

    fn sysnode(&self) -> Option<Arc<dyn aster_systree::SysNode>> {
        None
    }
}

/// Holds the metadata shared by every OQueue filesystem inode.
///
/// Concrete inodes embed a `Common` and forward the boilerplate `Inode` accessors to it via
/// `#[inherit_methods(from = "self.common")]`.
struct Common {
    metadata: RwLock<Metadata>,
    extension: Extension,
    fs: Weak<dyn FileSystem>,
}

impl Common {
    fn new(metadata: Metadata, fs: Weak<dyn FileSystem>) -> Self {
        Self {
            metadata: RwLock::new(metadata),
            extension: Extension::new(),
            fs,
        }
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.upgrade().unwrap()
    }

    fn metadata(&self) -> Metadata {
        *self.metadata.read()
    }

    fn ino(&self) -> u64 {
        self.metadata.read().ino
    }

    fn size(&self) -> usize {
        self.metadata.read().size
    }

    fn atime(&self) -> Duration {
        self.metadata.read().last_access_at
    }

    fn set_atime(&self, time: Duration) {
        self.metadata.write().last_access_at = time;
    }

    fn mtime(&self) -> Duration {
        self.metadata.read().last_modify_at
    }

    fn set_mtime(&self, time: Duration) {
        self.metadata.write().last_modify_at = time;
    }

    fn ctime(&self) -> Duration {
        self.metadata.read().last_meta_change_at
    }

    fn set_ctime(&self, time: Duration) {
        self.metadata.write().last_meta_change_at = time;
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

    fn extension(&self) -> &Extension {
        &self.extension
    }
}

#[cfg(ktest)]
mod tests {
    use ostd::{
        orpc::{
            oqueue::{ConsumableOQueue as _, ConsumableOQueueRef, OQueueBase as _, registry},
            path::{Path, PathComponent},
        },
        prelude::*,
    };

    use super::*;
    use crate::fs::file::{AccessMode, StatusFlags};

    /// Reads all currently-available bytes from a nonblocking stream handle.
    fn read_all(stream: &dyn crate::fs::file::FileIo) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut buf = [0u8; 512];
            let mut writer = VmWriter::from(&mut buf[..]).to_fallible();
            match stream.read_at(0, &mut writer, StatusFlags::O_NONBLOCK) {
                Ok(0) => break,
                Ok(len) => out.extend_from_slice(&buf[..len]),
                Err(_) => break, // `EAGAIN`: no more data is currently available.
            }
        }
        out
    }

    #[ktest]
    fn tree_and_observation_files() {
        // Inode metadata reads the real-time clock, which the ktest kernel does not boot.
        crate::time::clocks::init_for_ktest();

        // Export an OQueue at `oqfstest.queue[0]`.
        let path = Path::new(alloc::vec![
            PathComponent::Name("oqfstest"),
            PathComponent::Name("queue"),
            PathComponent::Index(0),
        ]);
        let queue = ConsumableOQueueRef::<usize>::new(16, path.clone());
        registry::register(&path, &queue.as_any_oqueue());

        let fs = OQueueFs::new();
        let root = fs.root_inode();

        // The namespace tree mirrors the path components down to the OQueue leaf directory.
        let namespace = root.lookup("oqfstest").unwrap();
        let queue_dir = namespace.lookup("queue").unwrap();
        let leaf = queue_dir.lookup("0").unwrap();
        assert!(root.lookup("nonexistent").is_err());

        // The leaf directory lists exactly the two observation files.
        let mut names = Vec::<String>::new();
        leaf.readdir_at(0, &mut names).unwrap();
        assert!(names.iter().any(|name| name == strong_observe::FILE_NAME));
        assert!(names.iter().any(|name| name == metadata::FILE_NAME));

        // `strong_observe` streams the values produced after it is opened.
        let stream = match leaf
            .lookup(strong_observe::FILE_NAME)
            .unwrap()
            .open(AccessMode::O_RDONLY, StatusFlags::O_NONBLOCK)
        {
            Some(Ok(stream)) => stream,
            _ => panic!("opening strong_observe should mint a stream handle"),
        };
        let producer = queue.attach_value_producer().unwrap();
        for value in [10usize, 20, 30] {
            producer.produce(value);
        }

        // The stream is records only (no header; that metadata lives in `metadata.yaml`).
        let bytes = read_all(stream.as_ref());
        let mut de = minicbor_serde::Deserializer::new(&bytes);
        let mut records = Vec::new();
        while de.decoder().position() < bytes.len() {
            let value: u64 = serde::Deserialize::deserialize(&mut de).unwrap();
            records.push(value);
        }
        assert_eq!(records, [10, 20, 30]);

        // `metadata.yaml` reports the queue's path and message type.
        let meta = leaf.lookup(metadata::FILE_NAME).unwrap();
        let mut buf = [0u8; 256];
        let mut writer = VmWriter::from(&mut buf[..]).to_fallible();
        let len = meta.read_at(0, &mut writer, StatusFlags::empty()).unwrap();
        let text = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(text.contains("oqfstest.queue[0]"));
        assert!(text.contains(core::any::type_name::<usize>()));

        // Once the queue is gone, the leaf disappears from the tree.
        drop(stream);
        drop(producer);
        drop(queue);
        assert!(root.lookup("oqfstest").is_err());
    }
}
