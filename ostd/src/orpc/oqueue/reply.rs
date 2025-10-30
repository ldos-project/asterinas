//! A specialized OQueue implementation used only for replies to requests. These OQueues are
//! expected to be transient.

use alloc::boxed::Box;

use crate::orpc::oqueue::{Consumer, OQueue, OQueueAttachError, Producer};

/// The OQueue implementation to use for ephemeral reply queues.
pub type ReplyQueue<T> = super::locking::LockingQueue<T>;

type ReplyHandlePair<T> = (Box<dyn Producer<T>>, Box<dyn Consumer<T>>);

impl<T: Send + 'static> ReplyQueue<T> {
    /// Construct a producer/consumer pair for handling an async message reply.
    pub fn new_pair() -> Result<ReplyHandlePair<T>, OQueueAttachError> {
        let oqueue = ReplyQueue::new(2);
        Ok((oqueue.attach_producer()?, oqueue.attach_consumer()?))
    }
}
