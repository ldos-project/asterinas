// SPDX-License-Identifier: MPL-2.0

//! The `metadata.yaml` file: a human-readable description of an OQueue.
//!
//! It is a regular file whose contents are regenerated on each read from the live export registry,
//! reporting the queue's path (`name`/`oqueues`) and message `type_name`.

use core::time::Duration;

use aster_util::printer::VmPrinter;
use inherit_methods_macro::inherit_methods;
use ostd::orpc::{oqueue::registry, path::Path};

use super::{BLOCK_SIZE, Common, OQueueFs};
use crate::{
    fs::{
        file::{InodeMode, InodeType, StatusFlags, mkmod},
        vfs::{
            file_system::FileSystem,
            inode::{Extension, Inode, InodeIo, Metadata},
        },
    },
    prelude::*,
    process::{Gid, Uid},
};

/// The name of the metadata file inside each OQueue directory.
pub(super) const FILE_NAME: &str = "metadata.yaml";

/// The inode for a `metadata.yaml` file.
pub(super) struct MetadataInode {
    /// The OQueue path this file describes.
    path: Path,
    common: Common,
}

/// Creates a `metadata.yaml` inode for the OQueue at `path`.
pub(super) fn new_inode(fs: Weak<OQueueFs>, path: Path) -> Arc<dyn Inode> {
    let oqueue_fs = fs.upgrade().unwrap();
    let ino = oqueue_fs.alloc_id();
    let metadata = Metadata::new_file(
        ino,
        mkmod!(a+r),
        BLOCK_SIZE,
        oqueue_fs.sb().container_dev_id,
    );
    let fs: Weak<dyn FileSystem> = fs;
    Arc::new(MetadataInode {
        path,
        common: Common::new(metadata, fs),
    })
}

impl MetadataInode {
    /// Renders the YAML description, skipping the first `offset` bytes for the current read.
    fn render(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        let path = self.path.to_string();
        let type_name = registry::lookup_export(&self.path)
            .map(|export| export.type_name())
            .unwrap_or("<unregistered>");

        let mut printer = VmPrinter::new_skip(writer, offset);
        writeln!(printer, "name: {}", path)?;
        writeln!(printer, "oqueues:")?;
        writeln!(printer, "  - {}", path)?;
        writeln!(printer, "type_name: {}", type_name)?;
        Ok(printer.bytes_written())
    }
}

impl InodeIo for MetadataInode {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        self.render(offset, writer)
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EPERM))
    }
}

#[inherit_methods(from = "self.common")]
impl Inode for MetadataInode {
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

    fn type_(&self) -> InodeType {
        InodeType::File
    }

    fn resize(&self, _new_size: usize) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }
}
