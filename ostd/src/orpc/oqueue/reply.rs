// SPDX-License-Identifier: MPL-2.0

//! A specialized OQueue implementation used only for replies to requests. These OQueues are
//! expected to be transient and can be optimized for that case.

use alloc::boxed::Box;

use crate::orpc::oqueue::{Consumer, OQueue, OQueueAttachError, Producer};

/// The OQueue implementation to use for ephemeral reply queues.
///
/// **NOTE:** This exists as an alias so that it can later be replaced with an optimized
/// implementation.
// pub type ReplyQueue<T> = super::locking::LockingQueue<T>;
pub type ReplyQueue<T> = super::ringbuffer::mpmc::MPMCOQueue<T, false, false>;

type ReplyHandlePair<T> = (Box<dyn Producer<T>>, Box<dyn Consumer<T>>);

impl<T: Send + 'static> ReplyQueue<T> {
    /// Construct a producer/consumer pair for handling an async message reply.
    pub fn new_pair() -> Result<ReplyHandlePair<T>, OQueueAttachError> {
        let oqueue = ReplyQueue::new(2, 0);
        Ok((oqueue.attach_producer()?, oqueue.attach_consumer()?))
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn test_send_message() {
        let (producer, consumer) = ReplyQueue::new_pair().unwrap();
        producer.produce(42);
        assert_eq!(consumer.consume(), 42);
    }
}
