// SPDX-License-Identifier: MPL-2.0

//! The OQueue filesystem (`/oqueue`).
//!
//! This pseudo-filesystem exposes OQueues that have been exported to userspace (see
//! [`ostd::orpc::oqueue::registry`]) as a procfs-like tree, so a userspace program can read the
//! in-kernel statistics flowing through them.
//!
//! This module currently provides the mountable skeleton with an empty root directory. The live
//! directory tree (mirroring the export registry) and the per-queue observation files are added in
//! later phases.

use core::time::Duration;

use inherit_methods_macro::inherit_methods;

use crate::{
    fs::{
        file::{InodeMode, InodeType, StatusFlags, mkmod},
        pseudofs::AnonDeviceId,
        utils::{DirentVisitor, NAME_MAX},
        vfs::{
            file_system::{FileSystem, FsEventSubscriberStats, SuperBlock},
            inode::{Extension, Inode, InodeIo, Metadata, MknodType},
            path::{FsPath, PathResolver, PerMountFlags, is_dot, is_dotdot},
            registry::{FsCreationCtx, FsProperties, FsType},
        },
    },
    prelude::*,
    process::{Gid, Uid},
};

/// Magic number for the OQueue filesystem (`"oque"`).
const OQUEUE_MAGIC: u64 = 0x6f71_7565;
/// Root inode ID.
const OQUEUE_ROOT_INO: u64 = 1;
/// Block size.
const BLOCK_SIZE: usize = 1024;

/// Registers the OQueue filesystem type so it can be mounted.
pub(super) fn init() {
    crate::fs::vfs::registry::register(&OQueueFsType).unwrap();
}

/// Mounts an OQueue filesystem at `/oqueue`, creating the mount point if needed.
///
/// Called during first-process initialization when the `oqfs` feature is enabled.
pub(in crate::fs) fn mount_root(path_resolver: &PathResolver, ctx: &Context) -> Result<()> {
    let root_path = path_resolver.lookup(&FsPath::try_from("/")?)?;
    let mount_point = root_path.new_fs_child("oqueue", InodeType::Dir, mkmod!(a+rx))?;
    mount_point.mount(
        OQueueFs::new(),
        PerMountFlags::default(),
        Some("oqueue".to_string()),
        ctx,
    )?;
    ostd::debug!("Mounted oqueuefs at \"/oqueue\"");
    Ok(())
}

struct OQueueFs {
    _anon_device_id: AnonDeviceId,
    sb: SuperBlock,
    root: Arc<dyn Inode>,
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
            root: RootDir::new_inode(weak_fs.clone(), &sb),
            fs_event_subscriber_stats: FsEventSubscriberStats::new(),
        })
    }
}

impl FileSystem for OQueueFs {
    fn name(&self) -> &'static str {
        "oqueue"
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

/// The inode at `/oqueue`.
///
/// The tree below it is empty for now; later phases populate it live from the export registry.
struct RootDir {
    this: Weak<RootDir>,
    common: Common,
}

impl RootDir {
    fn new_inode(fs: Weak<OQueueFs>, sb: &SuperBlock) -> Arc<dyn Inode> {
        let fs: Weak<dyn FileSystem> = fs;
        let metadata = Metadata::new_dir(
            OQUEUE_ROOT_INO,
            mkmod!(a+rx),
            BLOCK_SIZE,
            sb.container_dev_id,
        );
        Arc::new_cyclic(|weak_self| RootDir {
            this: weak_self.clone(),
            common: Common::new(metadata, fs),
        })
    }

    fn this(&self) -> Arc<dyn Inode> {
        self.this.upgrade().unwrap()
    }
}

impl InodeIo for RootDir {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }
}

#[inherit_methods(from = "self.common")]
impl Inode for RootDir {
    fn size(&self) -> usize;
    fn metadata(&self) -> Metadata;
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

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let this = self.this();

        // The root is its own parent, so `.` and `..` resolve to the same inode. There are no
        // other entries yet.
        let try_readdir = |cursor: &mut usize, visitor: &mut dyn DirentVisitor| -> Result<()> {
            for (name, next_offset) in [(".", 1usize), ("..", 2usize)] {
                if next_offset <= *cursor {
                    continue;
                }
                visitor.visit(name, this.ino(), InodeType::Dir, next_offset)?;
                *cursor = next_offset;
            }
            Ok(())
        };

        let mut cursor = offset;
        match try_readdir(&mut cursor, visitor) {
            Err(e) if cursor == offset => Err(e),
            _ => Ok(cursor - offset),
        }
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

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        if is_dot(name) || is_dotdot(name) {
            return Ok(self.this());
        }
        return_errno_with_message!(Errno::ENOENT, "the file does not exist");
    }

    fn rename(&self, _old_name: &str, _target: &Arc<dyn Inode>, _new_name: &str) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn seek_end(&self) -> Option<usize> {
        Some(0)
    }
}
