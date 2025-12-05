// SPDX-License-Identifier: MPL-2.0

//! A specialized OQueue implementation used only for replies to requests. These OQueues are
//! expected to be transient and can be optimized for that case.

use alloc::boxed::Box;

use crate::orpc::oqueue::{
    Consumer, OQueue, OQueueAttachError, OQueueRef, Producer, wrapper::TransformingQueueConsumer,
};

/// The OQueue implementation to use for ephemeral reply queues.
///
/// **NOTE:** This exists as an alias so that it can later be replaced with an optimized
/// implementation.
pub type ReplyQueue<T> = super::locking::ObservableLockingQueue<T>;

type ReplyHandlePair<T, U> = (Box<dyn Producer<T>>, Box<dyn Consumer<U>>);

impl<T: Clone + Send + 'static> ReplyQueue<T> {
    /// Construct a producer/consumer pair for handling an async message reply.
    pub fn new_pair_transformed<U: Send + 'static, F>(
        parent: Option<(&OQueueRef<U>, F)>,
    ) -> Result<ReplyHandlePair<T, T>, OQueueAttachError>
    where
        F: Fn(&T) -> U + Clone + Send + Sync + 'static,
    {
        let oqueue = ReplyQueue::new(2, 1);
        if let Some((parent, f)) = parent {
            parent.attach_child_queue(TransformingQueueConsumer::new(oqueue.clone(), f))?;
        }
        Ok((oqueue.attach_producer()?, oqueue.attach_consumer()?))
    }

    pub fn new_pair(
        parent: Option<&OQueueRef<T>>,
    ) -> Result<ReplyHandlePair<T, T>, OQueueAttachError> {
        let oqueue = ReplyQueue::new(2, 1);
        if let Some(parent) = parent {
            parent.attach_child_queue(oqueue.clone())?;
        }
        Ok((oqueue.attach_producer()?, oqueue.attach_consumer()?))
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{orpc::oqueue::locking::ObservableLockingQueue, prelude::*};

    #[ktest]
    fn test_send_message() {
        let (producer, consumer) = ReplyQueue::new_pair(None).unwrap();
        producer.produce(42);
        assert_eq!(consumer.consume(), 42);
    }

    #[ktest]
    fn test_send_message_some() {
        let parent: OQueueRef<i32> = ObservableLockingQueue::<i32>::new(2, 2);
        let parent_consumer = parent.attach_consumer().unwrap();
        let (producer, consumer) = ReplyQueue::new_pair(Some(&parent)).unwrap();
        producer.produce(42);
        assert_eq!(consumer.consume(), 42);
        assert_eq!(parent_consumer.consume(), 42);
    }

    #[ktest]
    fn test_send_message_transformed() {
        let parent: OQueueRef<i32> = ObservableLockingQueue::<i32>::new(2, 2);
        let parent_consumer = parent.attach_consumer().unwrap();
        let (producer, consumer) =
            ReplyQueue::new_pair_transformed::<i32, _>(Some((&parent, |x: &i32| x * 2))).unwrap();
        producer.produce(21i32);
        assert_eq!(consumer.consume(), 21i32);
        assert_eq!(parent_consumer.consume(), 42);
    }
}
