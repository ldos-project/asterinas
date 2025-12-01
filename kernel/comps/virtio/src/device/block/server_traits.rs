use alloc::boxed::Box;

use ostd::orpc::{
    oqueue::{
        OQueue as _, OQueueRef, Producer, locking::ObservableLockingQueue, reply::ReplyQueue,
    },
    orpc_trait,
};

use aster_block::bio::Bio;
use crate::device::VirtioDeviceError;
type Result<T> = core::result::Result<T, VirtioDeviceError>;
use ostd::orpc::{framework::errors::RPCError, oqueue::OQueueAttachError};

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

pub struct ORPCBioRequest {
    pub handle: Bio,
    /// A producer handle into an OQueue to send the reply to. If this is [`None`] no reply is sent.
    pub reply_handle: Option<Box<dyn Producer<Bio>>>,
}

impl From<Bio> for ORPCBioRequest {
    fn from(handle: Bio) -> Self {
        Self {
            handle,
            reply_handle: None,
        }
    }
}

#[orpc_trait]
pub trait ORPCBio {
    /// Process a Bio asynchronously. The reply will be sent to [`ORPCBioRequest::reply_handle`].
    fn process_bio_async(&self, handle: ORPCBioRequest) -> Result<()>;

    /// Process a request synchronously.
    fn process_bio(&self, handle: Bio) -> Result<()> {
        let reply_oqueue = ReplyQueue::new(2);
        let consumer = reply_oqueue.attach_consumer()?;
        self.process_bio_async(ORPCBioRequest {
            handle,
            reply_handle: Some(reply_oqueue.attach_producer()?),
        })?;
        consumer.consume();
        Ok(())
    }

    // /// Writes a page synchronously.
    // fn write_page(&self, handle: PageHandle) -> Result<()> {
    //     let reply_oqueue = ReplyQueue::new(2);
    //     let consumer = reply_oqueue.attach_consumer()?;
    //     self.write_page_async(AsyncWriteRequest {
    //         handle,
    //         reply_handle: Some(reply_oqueue.attach_producer()?),
    //     })?;
    //     consumer.consume();
    //     Ok(())
    // }
}

#[orpc_trait]
pub trait PageIOObservable {
    /// The OQueue containing every read request. This includes both sync and async reads on this
    /// trait and any other read operations on other traits (for instance,
    /// [`crate::vm::vmo::Pager::commit_page`]).
    fn page_reads_oqueue(&self) -> OQueueRef<usize> {
        ObservableLockingQueue::new(8, 8)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn page_writes_oqueue(&self) -> OQueueRef<usize> {
        ObservableLockingQueue::new(8, 8)
    }
}

#[orpc_trait]
pub trait OQueueSubmit {
    /// Enqueues a `Bio` via an OQueue. 
    fn observable_submit(&self, bio: Bio) -> Result<()>;
}