// SPDX-License-Identifier: MPL-2.0

#![allow(missing_docs)]
#![allow(unused)]

use alloc::sync::Arc;
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

use super::*;
use crate::{
    prelude::Vec,
    sync::{Mutex, WaitQueue},
    task::{Task, TaskOptions},
    timer::Jiffies,
};

#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub(crate) struct TestMessage {
    pub(crate) x: usize,
}

pub(crate) fn test_produce_consume(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();
    let consumer = queue.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_err());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);

    assert!(producer.try_produce(test_message).is_ok());
}

pub(crate) fn test_produce_strong_observe(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();
    let consumer = queue.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    // Normal operation when there is no observer
    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_err());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);

    assert!(producer.try_produce(test_message).is_ok());
    assert_eq!(consumer.consume(), test_message);

    assert_eq!(consumer.try_consume(), None);

    // With observer we should block sooner.
    let observer = queue
        .attach_strong_observer(ObservationQuery::new(|m: &TestMessage| m.x))
        .unwrap();

    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_err());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);
    assert!(
        producer.try_produce(test_message).is_err(),
        "send should fail here due to observer not having observed."
    );

    assert_eq!(observer.strong_observe().unwrap(), test_message.x);
    assert_eq!(observer.try_strong_observe().unwrap(), None);

    assert!(producer.try_produce(test_message).is_ok());
}

pub(crate) fn test_produce_weak_observe(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();
    let consumer = queue.attach_consumer().unwrap();
    let weak_observer = queue
        .attach_weak_observer(2, ObservationQuery::new(|m: &TestMessage| m.x))
        .unwrap();

    let newest_cursor = weak_observer.newest_cursor();
    assert_eq!(weak_observer.weak_observe(newest_cursor).unwrap(), None);

    let test_message = TestMessage { x: 42 };
    producer.produce(test_message);

    // Check recent cursor
    let newest_cursor = weak_observer.newest_cursor();
    assert_eq!(
        weak_observer.weak_observe(newest_cursor).unwrap(),
        Some(test_message.x)
    );

    // Check old cursor
    let oldest_cursor = weak_observer.oldest_cursor();
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor).unwrap(),
        Some(test_message.x)
    );

    assert_eq!(consumer.consume(), test_message);

    assert_eq!(weak_observer.newest_cursor(), newest_cursor);
    assert_eq!(weak_observer.oldest_cursor(), oldest_cursor);
    assert_eq!(
        weak_observer.weak_observe(newest_cursor).unwrap(),
        Some(test_message.x)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor).unwrap(),
        Some(test_message.x)
    );

    let test_message_2 = TestMessage { x: 43 };

    producer.produce(test_message_2);
    assert_eq!(consumer.consume(), test_message_2);

    let newest_cursor = weak_observer.newest_cursor();
    assert_eq!(
        weak_observer.weak_observe(newest_cursor).unwrap(),
        Some(test_message_2.x)
    );
    let oldest_cursor = weak_observer.oldest_cursor();
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor).unwrap(),
        Some(test_message.x)
    );

    let test_message_3 = TestMessage { x: 44 };

    producer.produce(test_message_3);

    assert_eq!(weak_observer.weak_observe(oldest_cursor).unwrap(), None);
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 1).unwrap(),
        Some(test_message_2.x)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 2).unwrap(),
        Some(test_message_3.x)
    );

    assert_eq!(consumer.consume(), test_message_3);
}

pub fn sleep(d: Duration) {
    let now = Jiffies::elapsed().as_duration();
    let target = now + d;
    while Jiffies::elapsed().as_duration() < target {
        Task::yield_now();
    }
}

/// Check that multithreading works at a basic level.
pub(crate) fn test_send_receive_blocker(
    queue: ConsumableOQueueRef<TestMessage>,
    n_messages: usize,
    n_observers: usize,
) {
    let wait_queue = Arc::new(WaitQueue::new());
    let completed_threads = Arc::new(AtomicUsize::new(0));

    // Consumer which receives all the messages
    let received_messages = Arc::new(Mutex::new(Vec::with_capacity(n_messages)));
    let _received_thread = TaskOptions::new({
        let consumer = queue.attach_consumer().unwrap();
        let received_messages = Arc::clone(&received_messages);
        let completed_threads = completed_threads.clone();
        let wait_queue = wait_queue.clone();
        move || {
            for i in 0..n_messages {
                let message = consumer.consume();
                assert_eq!(message.x, i);
                received_messages.lock().push(message);
            }
            completed_threads.fetch_add(1, Ordering::Relaxed);
            wait_queue.wake_all();
        }
    })
    .spawn()
    .unwrap();

    // Observers which strong observe all of the messages
    let _observers: Vec<_> = (0..n_observers)
        .map(|_| {
            let strong_observer = queue
                .attach_strong_observer(ObservationQuery::new(|m: &TestMessage| m.x))
                .unwrap();
            let completed_threads = completed_threads.clone();
            let wait_queue = wait_queue.clone();
            TaskOptions::new({
                move || {
                    for i in 0..n_messages {
                        let observed = strong_observer.strong_observe().unwrap();
                        assert_eq!(observed, i);
                    }
                    completed_threads.fetch_add(1, Ordering::Relaxed);
                    wait_queue.wake_all();
                }
            })
            .spawn()
            .unwrap();
        })
        .collect();

    // Producer thread which sends n messages
    let _producer_thread = TaskOptions::new({
        let producer = queue.attach_value_producer().unwrap();
        let completed_threads = completed_threads.clone();
        let wait_queue = wait_queue.clone();
        move || {
            for x in 0..n_messages {
                producer.produce(TestMessage { x });
                sleep(Duration::from_millis(3));
            }
            completed_threads.fetch_add(1, Ordering::Relaxed);
            wait_queue.wake_all();
        }
    })
    .spawn()
    .unwrap();

    // producer, consumer, and n_observers
    let n_threads = 1 + 1 + n_observers;
    wait_queue.wait_until(|| {
        if completed_threads.load(Ordering::Relaxed) == n_threads {
            Some(())
        } else {
            None
        }
    });

    let received_messages = received_messages.lock();
    assert_eq!(received_messages.len(), n_messages);
}

/// Test produce operations with strong observation, but without any consumer attached. Especially,
/// if the queue blocks when there is no consumer.
pub(crate) fn test_produce_strong_observe_only(queue: ConsumableOQueueRef<TestMessage>) {
    let test_message = TestMessage { x: 42 };

    let producer = queue.attach_value_producer().unwrap();
    let observer = queue
        .attach_strong_observer(ObservationQuery::new(|m: &TestMessage| m.x))
        .unwrap();

    for i in 0..100 {
        assert!(
            producer.try_produce(test_message).is_ok(),
            "failed to produce at iteration {i}"
        );
        assert_eq!(observer.strong_observe().unwrap(), test_message.x);
    }
}

pub(crate) fn test_consumer_late_attach(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();

    producer.produce(TestMessage { x: 1 });

    let consumer = queue.attach_consumer().unwrap();

    producer.produce(TestMessage { x: 2 });

    // Should only receive the second message because we attached after the first.
    assert_eq!(consumer.consume(), TestMessage { x: 2 });
    assert_eq!(consumer.try_consume(), None);
}

pub(crate) fn test_consumer_detach(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();
    let consumer = queue.attach_consumer().unwrap();

    producer.produce(TestMessage { x: 0 });
    assert_eq!(consumer.consume(), TestMessage { x: 0 });

    // Detach the consumer.
    drop(consumer);

    // The queue should no longer be blocked.
    for i in 0..100 {
        assert!(
            producer.try_produce(TestMessage { x: i }).is_ok(),
            "failed at {}",
            i
        );
    }
}

pub(crate) fn test_strong_observer_late_attach(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();

    producer.produce(TestMessage { x: 1 });

    let observer = queue
        .attach_strong_observer(ObservationQuery::new(|m: &TestMessage| m.x))
        .unwrap();

    producer.produce(TestMessage { x: 2 });

    // Should only observe the second message because we attached after the first.
    assert_eq!(observer.strong_observe().unwrap(), 2);
    assert_eq!(observer.try_strong_observe().unwrap(), None);
}

pub(crate) fn test_strong_observer_detach(queue: ConsumableOQueueRef<TestMessage>) {
    let producer = queue.attach_value_producer().unwrap();
    let observer = queue
        .attach_strong_observer(ObservationQuery::new(|m: &TestMessage| m.x))
        .unwrap();

    producer.produce(TestMessage { x: 0 });
    assert_eq!(observer.strong_observe().unwrap(), 0);

    // Detach the observer.
    drop(observer);

    // The queue should no longer be blocked.
    for i in 0..100 {
        assert!(
            producer.try_produce(TestMessage { x: i }).is_ok(),
            "failed at {}",
            i
        );
    }
}
