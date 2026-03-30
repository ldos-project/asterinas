// SPDX-License-Identifier: MPL-2.0

use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};
use ostd::orpc::{
    errors::RPCError,
    oqueue::{ConsumableOQueue as _, ConsumableOQueueRef, OQueue as _, OQueueError, OQueueRef},
    orpc_trait,
};

use crate::device::VirtioDeviceError;

impl From<RPCError> for VirtioDeviceError {
    fn from(value: RPCError) -> Self {
        VirtioDeviceError::RPCError(value)
    }
}

impl From<OQueueError> for VirtioDeviceError {
    fn from(value: OQueueError) -> Self {
        VirtioDeviceError::OQueueError(value)
    }
}

#[orpc_trait]
pub trait BlockIOObservable {
    /// The OQueue containing every bio submission request.
    /// The submission queue doesn't needed to be observable.
    fn bio_submission_oqueue(&self) -> ConsumableOQueueRef<SubmittedBio> {
        ConsumableOQueueRef::new_anonymous(32)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn bio_completion_oqueue(&self) -> OQueueRef<BlockDeviceCompletionStats> {
        OQueueRef::new_anonymous(4096)
    }
}

/// A unique identifier for tracking I/O requests.
pub type IoRequestId = u64;
