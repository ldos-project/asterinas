use alloc::boxed::Box;

use ostd::orpc::{
    oqueue::{
        OQueue as _, OQueueRef, Producer, locking::ObservableLockingQueue, reply::ReplyQueue, locking::LockingQueue
    },
    orpc_trait,
};

use aster_block::bio::{SubmittedBio, BlockDeviceCompletionTrace};
use crate::device::VirtioDeviceError;
type Result<T> = core::result::Result<T, VirtioDeviceError>;
use ostd::orpc::{framework::errors::RPCError, oqueue::OQueueAttachError};
use ostd::timer::Jiffies;

impl From<RPCError> for VirtioDeviceError {
    fn from(value: RPCError) -> Self {
        match value {
            RPCError::Panic { message: _ } => {
                VirtioDeviceError::ORPCServerPanicked
            }
            RPCError::ServerMissing => {
                VirtioDeviceError::ORPCServerMissing
            }
        }
    }
}

impl From<OQueueAttachError> for VirtioDeviceError {
    fn from(value: OQueueAttachError) -> Self {
        match value {
            OQueueAttachError::Unsupported { .. } => {
                VirtioDeviceError::OQueueAttachmentUnsupported
            }
            OQueueAttachError::AllocationFailed { .. } => {
                VirtioDeviceError::OQueueAttachmentAllocationFailed
            }
        }
    }
}

// pub struct ORPCBioRequest {
//     pub handle: SubmittedBio,
//     /// A producer handle into an OQueue to send the reply to. If this is [`None`] no reply is sent.
//     pub reply_handle: Option<Box<dyn Producer<BlockDeviceCompletionTrace>>>,
//     pub submission_time: Option<SystemTime>,
//     pub outstandint_requests: Option<Box<BioRequestSingleQueue>>,
// }

// impl From<Bio> for ORPCBioRequest {
//     fn from(handle: Bio) -> Self {
//         Self {
//             handle,
//             reply_handle: None,
//             submission_time: None,
//             outstandint_requests: None,
//         }
//     }
// }

// #[orpc_trait]
// pub trait ORPCBio {
//     /// Process a Bio asynchronously. The reply will be sent to [`ORPCBioRequest::reply_handle`].
//     fn process_bio_async(&self, handle: ORPCBioRequest) -> Result<()>;

//     /// Process a request synchronously.
//     fn process_bio(&self, handle: SubmittedBio) -> Result<()> {
//         let reply_oqueue = ReplyQueue::new(2);
//         let consumer = reply_oqueue.attach_consumer()?;
//         self.process_bio_async(ORPCBioRequest {
//             handle,
//             reply_handle: Some(reply_oqueue.attach_producer()?),
//             submission_time: Some(SystemTime::now()),
//         })?;
//         consumer.consume();
//         Ok(())
//     }
// }

#[orpc_trait]
pub trait BlockIOObservable {
    /// The OQueue containing every bio submission request. 
    /// The submission queue doesn't needed to be observable. 
    fn bio_submission_oqueue(&self) -> OQueueRef<SubmittedBio> {
        LockingQueue::new(32)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn bio_completion_oqueue(&self) -> OQueueRef<BlockDeviceCompletionTrace> {
        ObservableLockingQueue::new(32, 1)
    }
}

/// A unique identifier for tracking I/O requests.
pub type IoRequestId = u64;

// /// ORPC trait for block device I/O performance tracing.
// ///
// /// This trait defines the interface for monitoring block device I/O performance,
// /// including submission/completion tracking and latency observation.
// #[orpc_trait]
// pub trait BlockDeviceTracerService {

//     /// Called when an I/O request is submitted.
//     ///
//     /// This increments the outstanding request counter and records the submission
//     /// timestamp for later latency calculation.
//     ///
//     /// Returns a unique request ID that should be passed to `on_completion` when
//     /// the request completes.
//     fn on_submission(&self) -> Result<IoRequestId>;

//     /// Called when an I/O request completes.
//     ///
//     /// This:
//     /// 1. Decrements the outstanding request counter
//     /// 2. Calculates the I/O latency from the submission timestamp
//     /// 3. Records the current outstanding count and latency to their respective queues
//     ///
//     /// # Arguments
//     /// * `request_id` - The ID returned by `on_submission` for this request
//     fn on_completion(&self, request_id: IoRequestId) -> Result<()>;

//     /// OQueue storing the performance traces of I/O requests.
//     ///
//     /// Consumers can attach to this queue to observe performance traces.
//     fn io_performance_traces(&self) -> OQueueRef<BlockDeviceTracerData> {
//         ObservableLockingQueue::new(4, 1)
//     }
// }