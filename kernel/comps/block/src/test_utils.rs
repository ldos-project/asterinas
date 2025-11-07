// SPDX-License-Identifier: MPL-2.0

#![cfg(ktest)]

use crate::{
    BlockDevice,
    bio::{BioEnqueueError, BioStatus, SubmittedBio},
};

/// A block device and immediately completes every submitted request.
#[derive(Debug)]
pub struct FakeBlockDevice;

impl BlockDevice for FakeBlockDevice {
    fn enqueue(&self, bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
        bio.complete(BioStatus::Complete);
        Ok(())
    }

    fn metadata(&self) -> crate::BlockDeviceMeta {
        todo!()
    }
}
