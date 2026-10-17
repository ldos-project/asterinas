// SPDX-License-Identifier: MPL-2.0

//! The `weak_observe` file: a newest-first snapshot of an OQueue's recent values.
//!
//! Opening the file attaches a weak observer. Each read at offset zero starts again from the newest
//! value retained since that open and emits a self-delimiting CBOR stream. Values are never split
//! across reads: a value is emitted only when its complete CBOR record fits in the caller's
//! remaining buffer. Weak observation is non-consuming, so repeated offset-zero reads return fresh
//! snapshots of the same history.

use core::time::Duration;

use inherit_methods_macro::inherit_methods;
use ostd::orpc::{
    oqueue::{CborWeakReader, registry},
    path::Path,
};

use super::{BLOCK_SIZE, Common, OQueueFs};
use crate::{
    events::IoEvents,
    fs::{
        file::{AccessMode, InodeMode, InodeType, PerOpenFileOps, StatusFlags, mkmod},
        vfs::{
            file_system::FileSystem,
            inode::{Extension, FileOps, Inode, Metadata},
        },
    },
    prelude::*,
    process::{
        Gid, Uid,
        signal::{PollHandle, Pollable},
    },
};

/// The name of the weak-observation snapshot file inside each observable OQueue directory.
pub(super) const FILE_NAME: &str = "weak_observe";

/// The inode for a `weak_observe` file.
pub(super) struct WeakObserveInode {
    /// The OQueue path this file reads from.
    path: Path,
    common: Common,
}

/// Creates a `weak_observe` inode for the OQueue at `path`.
pub(super) fn new_inode(fs: Weak<OQueueFs>, path: Path) -> Arc<dyn Inode> {
    let oqueue_fs = fs.upgrade().unwrap();
    let ino = oqueue_fs.alloc_id();
    let metadata = Metadata::new_file(
        ino,
        mkmod!(u+r),
        BLOCK_SIZE,
        oqueue_fs.sb().container_dev_id,
    );
    Arc::new(WeakObserveInode {
        path,
        common: Common::new(metadata, fs),
    })
}

impl FileOps for WeakObserveInode {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EIO))
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(
            Errno::EPERM,
            "the OQueue weak-observation file is read-only"
        )
    }
}

/// A per-open weak observer over an OQueue's recent values.
struct WeakObserveFile {
    observer: Mutex<Box<dyn CborWeakReader>>,
}

impl FileOps for WeakObserveFile {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if offset != 0 {
            return_errno_with_message!(
                Errno::EINVAL,
                "an OQueue weak-observation read must start at offset zero"
            );
        }
        if !writer.has_avail() {
            return Ok(0);
        }

        let observer = self.observer.lock();
        let mut cursor = observer.newest_cursor();
        let initial_avail = writer.avail();

        loop {
            let mut record = Vec::new();
            match observer.observe_into(cursor, &mut record) {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
            if record.len() > writer.avail() {
                break;
            }

            writer.write_fallible(&mut VmReader::from(record.as_slice()))?;
            cursor = cursor - 1;
        }

        Ok(initial_avail - writer.avail())
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(
            Errno::EPERM,
            "the OQueue weak-observation file is read-only"
        )
    }
}

impl PerOpenFileOps for WeakObserveFile {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }
}

impl Pollable for WeakObserveFile {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        IoEvents::IN & mask
    }
}

#[inherit_methods(from = "self.common")]
impl Inode for WeakObserveInode {
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

    fn type_(&self) -> InodeType {
        InodeType::File
    }

    fn resize(&self, _new_size: usize) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }

    fn open(
        &self,
        access_mode: AccessMode,
        _status_flags: StatusFlags,
    ) -> Option<Result<Box<dyn PerOpenFileOps>>> {
        if access_mode.is_writable() {
            return Some(Err(Error::with_message(
                Errno::EPERM,
                "the OQueue weak-observation file is read-only",
            )));
        }
        let result = registry::lookup_export(&self.path)
            .ok_or_else(|| Error::new(Errno::ENOENT))
            .and_then(|export| {
                export.attach_weak_observer().map_err(|_| {
                    Error::with_message(Errno::ENODEV, "failed to attach an OQueue weak observer")
                })
            })
            .map(|observer| {
                Box::new(WeakObserveFile {
                    observer: Mutex::new(observer),
                }) as Box<dyn PerOpenFileOps>
            });
        Some(result)
    }
}
