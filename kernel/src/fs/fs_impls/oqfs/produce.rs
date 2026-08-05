// SPDX-License-Identifier: MPL-2.0

//! The `produce` file: lets userspace produce values into an OQueue.
//!
//! Opening this file attaches a fresh producer to the OQueue, so writes to it are decoded as CBOR
//! records and produced into the queue by value. Only OQueues exported via
//! [`ostd::orpc::oqueue::registry::register_producible`] support this; every other export reports
//! [`ostd::orpc::oqueue::OQueueError::Unsupported`] on `open`, matching `attach_producer`'s default.
//!
//! The file behaves like a pipe in the write direction: writes are producing and offset-free
//! (`lseek` fails with `ESPIPE`), and a blocking write waits for space in the OQueue.

use alloc::{boxed::Box, vec::Vec};
use core::time::Duration;

use inherit_methods_macro::inherit_methods;
use ostd::{
    orpc::{
        oqueue::{CborProducer, ProduceCborError, registry},
        path::Path,
    },
    sync::Mutex,
};

use super::{BLOCK_SIZE, Common, OQueueFs};
use crate::{
    events::IoEvents,
    fs::{
        file::{AccessMode, FileIo, InodeMode, InodeType, StatusFlags, mkmod},
        vfs::{
            file_system::FileSystem,
            inode::{Extension, Inode, InodeIo, Metadata},
        },
    },
    prelude::*,
    process::{
        Gid, Uid,
        signal::{PollHandle, Pollable, Pollee},
    },
};

/// The name of the produce file inside each OQueue directory.
pub(super) const FILE_NAME: &str = "produce";

/// Upper bound on the bytes buffered per open handle while waiting for a complete CBOR record.
const MAX_PENDING_BYTES: usize = 64 * 1024;

/// The inode for a `produce` file.
///
/// It is a regular file whose [`Inode::open`] mints a fresh per-open [`FileIo`] handle, so no
/// device registration under `/dev` is needed.
pub(super) struct ProduceInode {
    /// The OQueue path this file produces into.
    path: Path,
    common: Common,
}

/// Creates a `produce` inode for the OQueue at `path`.
pub(super) fn new_inode(fs: Weak<OQueueFs>, path: Path) -> Arc<dyn Inode> {
    let oqueue_fs = fs.upgrade().unwrap();
    let ino = oqueue_fs.alloc_id();
    // Security is enforced by permissions rather than by the export's direction: `strong_observe`
    // and `produce` files are both owner-only, and `Metadata::new_file` always sets the owner to
    // root, so only root (or root-owned processes) can open either.
    let metadata = Metadata::new_file(
        ino,
        mkmod!(u+w),
        BLOCK_SIZE,
        oqueue_fs.sb().container_dev_id,
    );
    Arc::new(ProduceInode {
        path,
        common: Common::new(metadata, fs),
    })
}

impl ProduceInode {
    /// Attaches a fresh producer to the OQueue and builds a per-open write handle.
    fn open_producer(&self, access_mode: AccessMode) -> Result<Box<dyn FileIo>> {
        if !access_mode.is_writable() {
            return_errno_with_message!(Errno::EPERM, "the OQueue produce file is write-only");
        }
        let export =
            registry::lookup_export(&self.path).ok_or_else(|| Error::new(Errno::ENOENT))?;
        let producer = export.attach_producer().map_err(|_| {
            Error::with_message(
                Errno::ENODEV,
                "failed to attach an OQueue producer, or this OQueue does not support producing",
            )
        })?;
        Ok(Box::new(ProduceFile::new(producer)))
    }
}

impl InodeIo for ProduceInode {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EPERM))
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        // This inode does not support `write_at`; the per-open handle minted by `open` does.
        Err(Error::new(Errno::EIO))
    }
}

#[inherit_methods(from = "self.common")]
impl Inode for ProduceInode {
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

    fn open(
        &self,
        access_mode: AccessMode,
        _status_flags: StatusFlags,
    ) -> Option<Result<Box<dyn FileIo>>> {
        Some(self.open_producer(access_mode))
    }
}

/// A per-open write handle over an OQueue's producer.
struct ProduceFile {
    /// The type-erased producer accepting CBOR records. Behind a mutex because it is not `Sync`;
    /// the lock is a sleeping mutex, so blocking under it (in `producer.produce_cbor`, which may
    /// block waiting for OQueue space) only serializes writers of this same open handle.
    producer: Mutex<Box<dyn CborProducer>>,
    /// Bytes written but not yet decoded into a complete CBOR record.
    pending: Mutex<Vec<u8>>,
    pollee: Pollee,
}

impl ProduceFile {
    fn new(producer: Box<dyn CborProducer>) -> Self {
        Self {
            producer: Mutex::new(producer),
            pending: Mutex::new(Vec::new()),
            pollee: Pollee::new(),
        }
    }

    /// Appends `bytes` to the pending buffer, then decodes and produces as many complete CBOR
    /// records as are available.
    fn produce_from(&self, bytes: &[u8]) -> Result<()> {
        let mut pending = self.pending.lock();
        pending.extend_from_slice(bytes);

        let producer = self.producer.lock();
        loop {
            let consumed = producer.produce_cbor(&pending).map_err(|err| match err {
                ProduceCborError::OQueue { .. } => Error::new(Errno::ENODEV),
                ProduceCborError::Malformed { .. } | ProduceCborError::SchemaMismatch { .. } => {
                    Error::with_message(
                        Errno::EINVAL,
                        "the OQueue produce file received a record that could not be decoded",
                    )
                }
                _ => Error::new(Errno::ENODEV),
            })?;
            if consumed == 0 {
                break;
            }
            pending.drain(..consumed);
        }

        if pending.len() > MAX_PENDING_BYTES {
            pending.clear();
            return_errno_with_message!(
                Errno::EINVAL,
                "the OQueue produce file received a record too large to decode"
            );
        }
        Ok(())
    }
}

impl InodeIo for ProduceFile {
    /// This is the offset-free write function; see the analogous comment on
    /// `strong_observe::StrongObserveFile::read_at`.
    ///
    /// `_status_flags` (`O_NONBLOCK`) is intentionally not consulted: a write blocks until the
    /// OQueue has space rather than returning `EAGAIN`, which is fine given a produce-exposed
    /// OQueue's single dedicated producer (see `registry::register_producible`).
    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        assert_eq!(offset, 0, "the OQueue produce file is offset-free");
        if !reader.has_remain() {
            return Ok(0);
        }

        let remain = reader.remain();
        let mut chunk = alloc::vec![0u8; remain];
        let copied = {
            let mut chunk_writer = VmWriter::from(&mut chunk[..]).to_fallible();
            chunk_writer.write_fallible(reader)?
        };

        self.produce_from(&chunk[..copied])?;
        self.pollee.notify(IoEvents::OUT);
        Ok(copied)
    }

    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EPERM, "the OQueue produce file is write-only")
    }
}

impl FileIo for ProduceFile {
    fn check_seekable(&self) -> Result<()> {
        return_errno_with_message!(Errno::ESPIPE, "the OQueue produce file is not seekable")
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

impl Pollable for ProduceFile {
    fn poll(&self, mask: IoEvents, poller: Option<&mut PollHandle>) -> IoEvents {
        self.pollee.poll_with(mask, poller, || IoEvents::OUT)
    }
}
