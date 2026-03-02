// SPDX-License-Identifier: MPL-2.0

use crate::orpc::oqueue::{ConsumableOQueue as _, ConsumableOQueueRef, Consumer, ValueProducer};

/// Create a producer-consumer pair for a single message.
///
/// This is for use as a reply channel for commands or calls. The returned producer may panic if the
/// user tried to produce more than once.
pub fn new_reply_pair<T: Send + 'static>() -> (ValueProducer<T>, Consumer<T>) {
    let reply = ConsumableOQueueRef::new(2);
    let reply_consumer = reply
        .attach_consumer()
        .expect("new reply OQueue always allows consumer");
    let reply_producer = reply
        .attach_value_producer()
        .expect("new reply OQueue always allows value producer");
    (reply_producer, reply_consumer)
}
