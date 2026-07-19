// SPDX-License-Identifier: MPL-2.0

//! The `strong_observe` file: a per-open stream of an OQueue's values.
//!
//! Opening this file attaches a fresh strong observer to the OQueue, so each reader receives its
//! own complete copy of every value produced after `open`. The bytes are a self-delimiting CBOR
//! stream of one record per observed value. Descriptive information about the queue (name and
//! message type) lives in the sibling `metadata.yaml`, not in this stream.
//!
//! The file behaves like a pipe: reads are consuming and offset-free (`lseek` fails with
//! `ESPIPE`), a blocking read waits for the next value, and `O_NONBLOCK` reads return `EAGAIN`
//! when idle.

// TODO: Distinguish the two stream-termination causes with distinct errnos instead of reporting
// `0` (EOF) for both. Return `ESTALE` when the observer is revoked (the reader fell too far behind
// but the OQueue still exists, so reopening resumes the stream), and `EIO` when the underlying
// OQueue disappears (is unregistered). This lets a reader tell whether to reconnect or give up.

use alloc::{boxed::Box, vec::Vec};
use core::time::Duration;

use inherit_methods_macro::inherit_methods;
use ostd::{
    orpc::{
        oqueue::{CborStrongObserve, registry},
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

/// The name of the streaming observation file inside each OQueue directory.
pub(super) const FILE_NAME: &str = "strong_observe";

/// Upper bound on the bytes buffered per open handle before draining pauses.
// TODO(Phase 5): replace with the `oqfs.buffer_bytes` kernel command-line knob.
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// The inode for a `strong_observe` file.
///
/// It is a regular file whose [`Inode::open`] mints a fresh per-open [`FileIo`] stream, so no
/// device registration under `/dev` is needed.
pub(super) struct StrongObserveInode {
    /// The OQueue path this file observes.
    path: Path,
    common: Common,
}

/// Creates a `strong_observe` inode for the OQueue at `path`.
pub(super) fn new_inode(fs: Weak<OQueueFs>, path: Path) -> Arc<dyn Inode> {
    let oqueue_fs = fs.upgrade().unwrap();
    let ino = oqueue_fs.alloc_id();
    let metadata = Metadata::new_file(
        ino,
        mkmod!(a+r),
        BLOCK_SIZE,
        oqueue_fs.sb().container_dev_id,
    );
    Arc::new(StrongObserveInode {
        path,
        common: Common::new(metadata, fs),
    })
}

impl StrongObserveInode {
    /// Attaches a fresh strong observer to the OQueue and builds a per-open streaming handle.
    fn open_stream(&self, access_mode: AccessMode) -> Result<Box<dyn FileIo>> {
        if access_mode.is_writable() {
            return_errno_with_message!(Errno::EPERM, "the OQueue stream is read-only");
        }
        let export =
            registry::lookup_export(&self.path).ok_or_else(|| Error::new(Errno::ENOENT))?;
        let observer = export.attach_strong_observer().map_err(|_| {
            Error::with_message(Errno::ENODEV, "failed to attach an OQueue observer")
        })?;
        Ok(Box::new(StrongObserveFile::new(observer)))
    }
}

impl InodeIo for StrongObserveInode {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        // This inode does not support `read_at`; the per-open handle minted by `open` does.
        Err(Error::new(Errno::EIO))
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
impl Inode for StrongObserveInode {
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
        Some(self.open_stream(access_mode))
    }
}

/// A per-open streaming handle over an OQueue's strong observer.
struct StrongObserveFile {
    /// The type-erased observer draining the OQueue. Behind a mutex because it is not `Sync`; the
    /// lock is a sleeping mutex, so blocking under it (in `strong_observe_into`) only serializes readers
    /// of this same open handle.
    observer: Mutex<Box<dyn CborStrongObserve>>,
    /// Encoded record bytes staged for delivery.
    buffer: Mutex<BufferState>,
    pollee: Pollee,
}

struct BufferState {
    bytes: Vec<u8>,
    /// Read cursor into `bytes`.
    pos: usize,
    /// Set once the OQueue is gone; subsequent reads report EOF.
    ended: bool,
}

impl StrongObserveFile {
    fn new(observer: Box<dyn CborStrongObserve>) -> Self {
        Self {
            observer: Mutex::new(observer),
            buffer: Mutex::new(BufferState {
                bytes: Vec::new(),
                pos: 0,
                ended: false,
            }),
            pollee: Pollee::new(),
        }
    }

    /// Drains encoded records from the observer into `out`, up to [`MAX_BUFFER_BYTES`].
    ///
    /// If `blocking` and nothing is immediately available, blocks for one record. Returns whether
    /// the stream has ended (the OQueue is gone or the observer was detached).
    fn drain(&self, out: &mut Vec<u8>, blocking: bool) -> bool {
        let observer = self.observer.lock();
        let mut ended = false;
        loop {
            match observer.try_strong_observe_into(out) {
                Ok(true) if out.len() < MAX_BUFFER_BYTES => continue,
                Ok(true) => break,
                Ok(false) => break,
                Err(_) => {
                    // Mark the observer as ended if the underlying OQueue every returns an error.
                    ended = true;
                    break;
                }
            }
        }
        if out.is_empty() && !ended && blocking && observer.strong_observe_into(out).is_err() {
            ended = true;
        }
        ended
    }

    /// Drains available records from the observer and appends them to the buffer.
    fn refill(&self, blocking: bool) -> bool {
        let mut records = Vec::new();
        let ended = self.drain(&mut records, blocking);
        let mut buffer = self.buffer.lock();
        buffer.bytes.extend_from_slice(&records);
        if ended {
            buffer.ended = true;
        }
        ended
    }

    /// Serves buffered bytes to `writer`, refilling once from the observer if the buffer is empty.
    ///
    /// Returns the number of bytes copied (`0` means EOF), or `EAGAIN` when nonblocking and idle.
    fn read_stream(&self, writer: &mut VmWriter, blocking: bool) -> Result<usize> {
        loop {
            {
                let mut buffer = self.buffer.lock();
                if buffer.pos < buffer.bytes.len() {
                    let mut reader = VmReader::from(&buffer.bytes[buffer.pos..]);
                    let copied = writer.write_fallible(&mut reader)?;
                    buffer.pos += copied;
                    if buffer.pos >= buffer.bytes.len() {
                        buffer.bytes.clear();
                        buffer.pos = 0;
                    }
                    return Ok(copied);
                }
                if buffer.ended {
                    return Ok(0);
                }
            }

            // The buffer is empty and the stream has not ended: refill from the observer.
            self.refill(blocking);

            let idle = {
                let buffer = self.buffer.lock();
                buffer.pos >= buffer.bytes.len() && !buffer.ended
            };

            if idle {
                // A blocking `drain` never returns idle, so this is the nonblocking case.
                return_errno_with_message!(Errno::EAGAIN, "no OQueue data is available");
            }
            self.pollee.notify(IoEvents::IN);
        }
    }

    /// Reports readability for `poll`, filling the buffer without blocking so the result reflects
    /// the currently-available data.
    fn check_events(&self) -> IoEvents {
        self.refill(false);
        let buffer = self.buffer.lock();
        if buffer.pos < buffer.bytes.len() || buffer.ended {
            IoEvents::IN
        } else {
            IoEvents::empty()
        }
    }
}

impl InodeIo for StrongObserveFile {
    /// This is the offset-free read function. It appears as read_at because
    /// the selection for calling the offseted and non-offseted read function
    /// happens in kernel/src/fs/file/inode_handle.rs:261, specifically the
    /// implementatino of read function in block "impl FileLike for InodeHandle"
    /// check the flag `is_offset_aware` and if not it just call the read_at(0,...).
    /// So the read_at here is effectively offset free.
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        // `is_offset_aware` is `false`, so the VFS always reads at offset 0 (see the doc comment
        // above); the stream itself has no notion of a position.
        assert_eq!(offset, 0, "the OQueue stream is offset-free");
        if !writer.has_avail() {
            return Ok(0);
        }
        let blocking = !status_flags.contains(StatusFlags::O_NONBLOCK);
        self.read_stream(writer, blocking)
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EPERM, "the OQueue stream is read-only")
    }
}

impl FileIo for StrongObserveFile {
    fn check_seekable(&self) -> Result<()> {
        return_errno_with_message!(Errno::ESPIPE, "the OQueue stream is not seekable")
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

impl Pollable for StrongObserveFile {
    fn poll(&self, mask: IoEvents, poller: Option<&mut PollHandle>) -> IoEvents {
        self.pollee.poll_with(mask, poller, || self.check_events())
    }
}
