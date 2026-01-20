// SPDX-License-Identifier: MPL-2.0

use ostd::{
    error::{InvalidArgsSnafu, IoSnafu},
    mm::{VmIo, VmReader, VmWriter},
};

use super::{
    BLOCK_SIZE, BlockDevice,
    bio::{Bio, BioEnqueueError, BioSegment, BioStatus, BioType, BioWaiter, SubmittedBio},
    id::{Bid, Sid},
};
use crate::{
    bio::{BioDirection, is_sector_aligned},
    prelude::*,
};

/// Implements several commonly used APIs for the block device to conveniently
/// read and write block(s).
// TODO: Add API to submit bio with multiple segments in scatter/gather manner.
impl dyn BlockDevice {
    /// Synchronously reads contiguous blocks starting from the `bid`.
    pub fn read_blocks(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
    ) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(
            BioType::Read,
            Sid::from(bid),
            vec![bio_segment],
            Some(general_complete_fn),
        );
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously reads contiguous blocks starting from the `bid`.
    pub fn read_blocks_async(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
    ) -> Result<BioWaiter, BioEnqueueError> {
        let bio = Bio::new(
            BioType::Read,
            Sid::from(bid),
            vec![bio_segment],
            Some(general_complete_fn),
        );
        bio.submit(self)
    }

    /// Asynchronously reads contiguous blocks starting from the `bid`. When complete call
    /// `complete_fn`.
    pub fn read_blocks_async_with_closure(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
        complete_fn: impl FnOnce(&SubmittedBio) + Send + 'static,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_with_closure(
            BioType::Read,
            Sid::from(bid),
            vec![bio_segment],
            complete_fn,
        );
        // The result of the operation in handled by the callback, so we can drop the waiter.
        let _ = bio.submit(self)?;
        Ok(())
    }

    /// Asynchronously reads contiguous segments starting from the `sid`. When complete call
    /// `complete_fn`.
    pub fn read_segments_async_with_closure(
        &self,
        sid: Sid,
        segments: Vec<BioSegment>,
        complete_fn: impl FnOnce(&SubmittedBio) + Send + 'static,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_with_closure(BioType::Read, sid, segments, complete_fn);
        let _ = bio.submit(self)?;
        Ok(())
    }

    /// Synchronously writes contiguous blocks starting from the `bid`.
    pub fn write_blocks(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
    ) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(
            BioType::Write,
            Sid::from(bid),
            vec![bio_segment],
            Some(general_complete_fn),
        );
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously writes contiguous blocks starting from the `bid`.
    pub fn write_blocks_async(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
    ) -> Result<BioWaiter, BioEnqueueError> {
        let bio = Bio::new(
            BioType::Write,
            Sid::from(bid),
            vec![bio_segment],
            Some(general_complete_fn),
        );
        bio.submit(self)
    }

    /// Asynchronously writes contiguous blocks starting from the `bid`. When complete call
    /// `complete_fn`.
    pub fn write_blocks_async_with_closure(
        &self,
        bid: Bid,
        bio_segment: BioSegment,
        complete_fn: impl FnOnce(&SubmittedBio) + Send + 'static,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_with_closure(
            BioType::Write,
            Sid::from(bid),
            vec![bio_segment],
            complete_fn,
        );
        // The result of the operation in handled by the callback, so we can drop the waiter.
        let _ = bio.submit(self)?;
        Ok(())
    }

    /// Asynchronously writes contiguous segments starting from the `sid`. When complete call
    /// `complete_fn`.
    pub fn write_segments_async_with_closure(
        &self,
        sid: Sid,
        segments: Vec<BioSegment>,
        complete_fn: impl FnOnce(&SubmittedBio) + Send + 'static,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_with_closure(BioType::Write, sid, segments, complete_fn);
        let _ = bio.submit(self)?;
        Ok(())
    }

    /// Issues a sync request
    pub fn sync(&self) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(
            BioType::Flush,
            Sid::from(Bid::from_offset(0)),
            vec![],
            Some(general_complete_fn),
        );
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }
}

impl VmIo for dyn BlockDevice {
    /// Reads consecutive bytes of several sectors in size.
    fn read(&self, offset: usize, writer: &mut VmWriter) -> ostd::Result<()> {
        let read_len = writer.avail();
        if !is_sector_aligned(offset) || !is_sector_aligned(read_len) {
            return InvalidArgsSnafu.fail();
        }
        if read_len == 0 {
            return Ok(());
        }

        let (bio, bio_segment) = {
            let num_blocks = {
                let first = Bid::from_offset(offset).to_raw();
                let last = Bid::from_offset(offset + read_len - 1).to_raw();
                (last - first + 1) as usize
            };
            let bio_segment = BioSegment::alloc_inner(
                num_blocks,
                offset % BLOCK_SIZE,
                read_len,
                BioDirection::FromDevice,
            );

            (
                Bio::new(
                    BioType::Read,
                    Sid::from_offset(offset),
                    vec![bio_segment.clone()],
                    Some(general_complete_fn),
                ),
                bio_segment,
            )
        };

        let status = bio.submit_and_wait(self)?;
        match status {
            BioStatus::Complete => bio_segment.read(0, writer),
            _ => IoSnafu.fail(),
        }
    }

    /// Writes consecutive bytes of several sectors in size.
    fn write(&self, offset: usize, reader: &mut VmReader) -> ostd::Result<()> {
        let write_len = reader.remain();
        if !is_sector_aligned(offset) || !is_sector_aligned(write_len) {
            return InvalidArgsSnafu.fail();
        }
        if write_len == 0 {
            return Ok(());
        }

        let bio = {
            let num_blocks = {
                let first = Bid::from_offset(offset).to_raw();
                let last = Bid::from_offset(offset + write_len - 1).to_raw();
                (last - first + 1) as usize
            };
            let bio_segment = BioSegment::alloc_inner(
                num_blocks,
                offset % BLOCK_SIZE,
                write_len,
                BioDirection::ToDevice,
            );
            bio_segment.write(0, reader)?;

            Bio::new(
                BioType::Write,
                Sid::from_offset(offset),
                vec![bio_segment],
                Some(general_complete_fn),
            )
        };

        let status = bio.submit_and_wait(self)?;
        match status {
            BioStatus::Complete => Ok(()),
            _ => IoSnafu.fail(),
        }
    }
}

impl dyn BlockDevice {
    /// Asynchronously writes consecutive bytes of several sectors in size.
    pub fn write_bytes_async(&self, offset: usize, buf: &[u8]) -> ostd::Result<BioWaiter> {
        let write_len = buf.len();
        if !is_sector_aligned(offset) || !is_sector_aligned(write_len) {
            return InvalidArgsSnafu.fail();
        }
        if write_len == 0 {
            return Ok(BioWaiter::new());
        }

        let bio = {
            let num_blocks = {
                let first = Bid::from_offset(offset).to_raw();
                let last = Bid::from_offset(offset + write_len - 1).to_raw();
                (last - first + 1) as usize
            };
            let bio_segment = BioSegment::alloc_inner(
                num_blocks,
                offset % BLOCK_SIZE,
                write_len,
                BioDirection::ToDevice,
            );
            bio_segment.write(0, &mut VmReader::from(buf).to_fallible())?;
            Bio::new(
                BioType::Write,
                Sid::from_offset(offset),
                vec![bio_segment],
                Some(general_complete_fn),
            )
        };

        let complete = bio.submit(self)?;
        Ok(complete)
    }
}

fn general_complete_fn(bio: &SubmittedBio) {
    match bio.status() {
        BioStatus::Complete => (),
        err_status => log::error!(
            "failed to do {:?} on the device with error status: {:?}",
            bio.type_(),
            err_status
        ),
    }
}

#[cfg(ktest)]
mod test {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use ostd::prelude::*;

    use super::*;
    use crate::{id::Bid, test_utils::FakeBlockDevice};

    /// An atomic counter used to track how many times a callback has been called. This must be
    /// static to work with static `fn`s passed to Bio.
    static CALLBACK_INVOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);

    /// Test asynchronous read_blocks with closure method.
    #[ktest]
    fn read_blocks_async_with_closure() {
        let bid = Bid::new(2);
        let bio_segment = BioSegment::alloc(1, BioDirection::FromDevice);
        let block_device: &dyn BlockDevice = &FakeBlockDevice;

        let complete_fn = |bio: &SubmittedBio| {
            assert_eq!(bio.type_(), BioType::Read);
            assert_eq!(bio.status(), BioStatus::Complete);
            CALLBACK_INVOCATION_COUNT.fetch_add(1, Ordering::SeqCst);
        };

        CALLBACK_INVOCATION_COUNT.store(0, Ordering::SeqCst);
        block_device
            .read_blocks_async_with_closure(bid, bio_segment, complete_fn)
            .unwrap();
        assert_eq!(CALLBACK_INVOCATION_COUNT.load(Ordering::SeqCst), 1);
    }

    /// Test asynchronous write_blocks with closure method.
    #[ktest]
    fn write_blocks_async_with_closure() {
        let bid = Bid::new(2);
        let bio_segment = BioSegment::alloc(1, BioDirection::ToDevice);
        let block_device: &dyn BlockDevice = &FakeBlockDevice;

        let complete_fn = |bio: &SubmittedBio| {
            assert_eq!(bio.type_(), BioType::Write);
            assert_eq!(bio.status(), BioStatus::Complete);
            CALLBACK_INVOCATION_COUNT.fetch_add(1, Ordering::SeqCst);
        };

        CALLBACK_INVOCATION_COUNT.store(0, Ordering::SeqCst);
        block_device
            .write_blocks_async_with_closure(bid, bio_segment, complete_fn)
            .unwrap();
        assert_eq!(CALLBACK_INVOCATION_COUNT.load(Ordering::SeqCst), 1);
    }
}
