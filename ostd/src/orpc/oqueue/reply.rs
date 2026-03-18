// SPDX-License-Identifier: MPL-2.0

//! A potentially specialized OQueue implementation used only for replies to requests. These OQueues
//! are expected to be transient and can be optimized for that case.
//!
//! (NOTE: Currently, this is just a normal OQueue.)

use crate::orpc::oqueue::{
    ConsumableOQueue, ConsumableOQueueRef, Consumer, OQueueError, ValueProducer,
};

/// The OQueue implementation to use for ephemeral reply queues.
///
/// (NOTE: This exists as an alias so that it can later be replaced with an optimized implementation
/// if needed.)
pub type ReplyQueue<T> = ConsumableOQueueRef<T>;

type ReplyHandlePair<T> = (ValueProducer<T>, Consumer<T>);

impl<T: Send + 'static> ReplyQueue<T> {
    /// Construct a producer/consumer pair for handling an async message reply.
    pub fn new_pair() -> Result<ReplyHandlePair<T>, OQueueError> {
        let oqueue = ConsumableOQueueRef::new_anonymous(2);
        Ok((oqueue.attach_value_producer()?, oqueue.attach_consumer()?))
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
