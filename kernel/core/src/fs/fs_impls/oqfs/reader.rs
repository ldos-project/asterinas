// SPDX-License-Identifier: MPL-2.0

//! The `strong_observe` and `consume` files: per-open streams of an OQueue's values.
//!
//! `strong_observe` opens attach a fresh strong observer to the OQueue, so each reader receives its
//! own complete copy of every value produced after `open`. `consume` opens attach a fresh consumer
//! instead. The bytes are a self-delimiting CBOR stream of one record per value. Descriptive
//! information about the queue (name and message type) lives in the sibling `metadata.yaml`, not in
//! this stream.
//!
//! The files behave like pipes: reads are consuming and offset-free (`lseek` fails with
//! `ESPIPE`), a blocking read waits for the next value, and `O_NONBLOCK` reads return `EAGAIN`
//! when idle.

// TODO: Distinguish the two stream-termination causes with distinct errnos instead of reporting
// `0` (EOF) for both. Return `ESTALE` when the observer is revoked (the reader fell too far behind
// but the OQueue still exists, so reopening resumes the stream), and `EIO` when the underlying
// OQueue disappears (is unregistered). This lets a reader tell whether to reconnect or give up.

use core::time::Duration;

use inherit_methods_macro::inherit_methods;
use ostd::orpc::{
    oqueue::{CborReader, registry},
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
        signal::{PollHandle, Pollable, Pollee},
    },
};

/// The name of the streaming observation file inside each OQueue directory.
pub(super) const STRONG_OBSERVE_FILE_NAME: &str = "strong_observe";

/// The name of the streaming consumption file inside each OQueue directory.
pub(super) const CONSUME_FILE_NAME: &str = "consume";

/// The direction of a reader stream file: whether opening it attaches a strong observer
/// (`strong_observe`) or a consumer (`consume`) to the OQueue.
pub(super) enum ReaderKind {
    /// The `strong_observe` file: a per-open copy of every value produced after `open`.
    StrongObserve,
    /// The `consume` file: exclusive delivery of the values to attached consumers.
    Consume,
}

/// Upper bound on the bytes buffered per open handle before draining pauses.
// TODO(Phase 5): replace with the `oqfs.buffer_bytes` kernel command-line knob.
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// The inode for a `strong_observe` or `consume` file.
///
/// It is a regular file whose [`Inode::open`] mints a fresh per-open [`FileIo`] stream, so no
/// device registration under `/dev` is needed.
pub(super) struct ReaderInode {
    /// The OQueue path this file reads from.
    path: Path,
    /// Whether opening the file attaches a strong observer (`strong_observe`) or a consumer
    /// (`consume`).
    kind: ReaderKind,
    common: Common,
}

/// Creates a reader stream inode for the OQueue at `path`.
pub(super) fn new_inode(fs: Weak<OQueueFs>, path: Path, kind: ReaderKind) -> Arc<dyn Inode> {
    let oqueue_fs = fs.upgrade().unwrap();
    let ino = oqueue_fs.alloc_id();
    // Security is enforced by permissions rather than by the export's direction: `strong_observe`,
    // `consume`, and `produce` files are all owner-only, and `Metadata::new_file` always sets the
    // owner to root, so only root (or root-owned processes) can open any of them.
    let metadata = Metadata::new_file(
        ino,
        mkmod!(u+r),
        BLOCK_SIZE,
        oqueue_fs.sb().container_dev_id,
    );
    Arc::new(ReaderInode {
        path,
        kind,
        common: Common::new(metadata, fs),
    })
}

impl ReaderInode {
    /// Attaches a fresh reader (strong observer or consumer) to the OQueue and builds a per-open
    /// streaming handle.
    fn open_stream(&self, access_mode: AccessMode) -> Result<Box<dyn PerOpenFileOps>> {
        if access_mode.is_writable() {
            return_errno_with_message!(Errno::EPERM, "the OQueue stream is read-only");
        }
        let export =
            registry::lookup_export(&self.path).ok_or_else(|| Error::new(Errno::ENOENT))?;
        let reader = match self.kind {
            ReaderKind::StrongObserve => export.attach_strong_observer(),
            ReaderKind::Consume => export.attach_consumer(),
        }
        .map_err(|_| Error::with_message(Errno::ENODEV, "failed to attach an OQueue reader"))?;
        Ok(Box::new(ReaderFile::new(reader)))
    }
}

impl FileOps for ReaderInode {
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
impl Inode for ReaderInode {
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
        Some(self.open_stream(access_mode))
    }
}

/// A per-open streaming handle over an OQueue's reader (strong observer or consumer).
struct ReaderFile {
    /// The type-erased reader draining the OQueue. Behind a mutex because it is not `Sync`; the
    /// lock is a sleeping mutex, so blocking under it (in `read_into`) only serializes readers
    /// of this same open handle.
    reader: Mutex<Box<dyn CborReader>>,
    /// Encoded record bytes staged for delivery.
    buffer: Mutex<BufferState>,
    pollee: Pollee,
}

struct BufferState {
    bytes: Vec<u8>,
    /// Read cursor into `bytes`.
    pos: usize,
    /// Set once the stream terminates (the observer was revoked; a consume stream never ends,
    /// because a `Consumer` keeps the queue alive); subsequent reads report EOF.
    ended: bool,
}

impl ReaderFile {
    fn new(reader: Box<dyn CborReader>) -> Self {
        Self {
            reader: Mutex::new(reader),
            buffer: Mutex::new(BufferState {
                bytes: Vec::new(),
                pos: 0,
                ended: false,
            }),
            pollee: Pollee::new(),
        }
    }

    /// Drains encoded records from the reader into `out`, up to [`MAX_BUFFER_BYTES`].
    ///
    /// If `blocking` and nothing is immediately available, blocks for one record. Returns whether
    /// the stream has ended (the observer was revoked).
    fn drain(&self, out: &mut Vec<u8>, blocking: bool) -> bool {
        let reader = self.reader.lock();
        let mut ended = false;
        loop {
            match reader.try_read_into(out) {
                Ok(true) if out.len() < MAX_BUFFER_BYTES => continue,
                Ok(true) => break,
                Ok(false) => break,
                Err(_) => {
                    // Mark the stream as ended if the underlying reader ever returns an error.
                    ended = true;
                    break;
                }
            }
        }
        if out.is_empty() && !ended && blocking && reader.read_into(out).is_err() {
            ended = true;
        }
        ended
    }

    /// Drains available records from the reader and appends them to the buffer.
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

    /// Serves buffered bytes to `writer`, refilling once from the reader if the buffer is empty.
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

            // The buffer is empty and the stream has not ended: refill from the reader.
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

impl FileOps for ReaderFile {
    /// This is the offset-free read function. It appears as read_at because
    /// the selection for calling the offsetted and non-offsetted read function
    /// happens in kernel/src/fs/file/inode_handle.rs:261, specifically the
    /// implementation of read function in block "impl FileLike for InodeHandle"
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

impl PerOpenFileOps for ReaderFile {
    fn check_seekable(&self) -> Result<()> {
        return_errno_with_message!(Errno::ESPIPE, "the OQueue stream is not seekable")
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

impl Pollable for ReaderFile {
    fn poll(&self, mask: IoEvents, poller: Option<&mut PollHandle>) -> IoEvents {
        self.pollee.poll_with(mask, poller, || self.check_events())
    }
}
