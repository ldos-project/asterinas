// SPDX-License-Identifier: MPL-2.0

use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};
use ostd::{
    orpc::{
        oqueue::{ConsumableOQueueRef, OQueueRef},
        orpc_trait,
    },
    path,
};

#[orpc_trait]
pub trait BlockIOObservable {
    /// The OQueue containing every bio submission request.
    /// The submission queue doesn't needed to be observable.
    fn bio_submission_oqueue(&self) -> ConsumableOQueueRef<SubmittedBio> {
        ConsumableOQueueRef::new(32, path!(io.block_io.bio_submission[unique]))
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn bio_completion_oqueue(&self) -> OQueueRef<BlockDeviceCompletionStats> {
        OQueueRef::new(4096, path!(io.block_io.bio_completion[unique]))
    }
}

/// A unique identifier for tracking I/O requests.
pub type IoRequestId = u64;
