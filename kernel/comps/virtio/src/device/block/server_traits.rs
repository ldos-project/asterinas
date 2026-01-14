use alloc::boxed::Box;

use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};
use ostd::orpc::{
    oqueue::{
        OQueue as _, OQueueRef, Producer,
        locking::{LockingQueue, ObservableLockingQueue},
        reply::ReplyQueue,
    },
    orpc_trait,
};

use crate::device::VirtioDeviceError;
type Result<T> = core::result::Result<T, VirtioDeviceError>;
use ostd::{
    orpc::{framework::errors::RPCError, oqueue::OQueueAttachError},
    timer::Jiffies,
};

impl From<RPCError> for VirtioDeviceError {
    fn from(value: RPCError) -> Self {
        match value {
            RPCError::Panic { message: _ } => VirtioDeviceError::ORPCServerPanicked,
            RPCError::ServerMissing => VirtioDeviceError::ORPCServerMissing,
        }
    }
}

impl From<OQueueAttachError> for VirtioDeviceError {
    fn from(value: OQueueAttachError) -> Self {
        match value {
            OQueueAttachError::Unsupported { .. } => VirtioDeviceError::OQueueAttachmentUnsupported,
            OQueueAttachError::AllocationFailed { .. } => {
                VirtioDeviceError::OQueueAttachmentAllocationFailed
            }
        }
    }
}

#[orpc_trait]
pub trait BlockIOObservable {
    /// The OQueue containing every bio submission request.
    /// The submission queue doesn't needed to be observable.
    fn bio_submission_oqueue(&self) -> OQueueRef<SubmittedBio> {
        LockingQueue::new(32)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn bio_completion_oqueue(&self) -> OQueueRef<BlockDeviceCompletionStats> {
        ObservableLockingQueue::new(32, 1)
    }
}

/// A unique identifier for tracking I/O requests.
pub type IoRequestId = u64;
