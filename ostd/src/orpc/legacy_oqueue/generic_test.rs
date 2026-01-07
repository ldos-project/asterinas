// SPDX-License-Identifier: MPL-2.0
#![allow(missing_docs)]
#![allow(unused)]

use alloc::sync::Arc;
use core::{
    convert::AsRef,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

use super::{super::sync::select, *};
use crate::{
    arch::timer::TIMER_FREQ,
    orpc::legacy_oqueue::locking::LockingQueue,
    prelude::Vec,
    sync::{Mutex, WaitQueue},
    task::{Task, TaskOptions},
    timer::Jiffies,
};

#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub(crate) struct TestMessage {
    pub(crate) x: usize,
}

pub(crate) fn test_produce_consume<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let producer = oqueue.attach_producer().unwrap();
    let consumer = oqueue.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_some());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);

    assert_eq!(producer.try_produce(test_message), None);
}

pub(crate) fn test_direct_produce_consume<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let consumer = oqueue.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    oqueue.produce(test_message);
    assert!(oqueue.try_produce(test_message).unwrap().is_some());

    assert_eq!(consumer.consume(), test_message);
}

pub(crate) fn test_produce_strong_observe(oqueue: Arc<dyn OQueue<TestMessage>>) {
    let producer = oqueue.attach_producer().unwrap();
    let consumer = oqueue.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    // Normal operation when there is no observer
    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_some());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);

    assert_eq!(producer.try_produce(test_message), None);
    assert_eq!(consumer.consume(), test_message);

    assert_eq!(consumer.try_consume(), None);

    // With observer we should block sooner.
    let observer = oqueue.attach_strong_observer().unwrap();

    producer.produce(test_message);
    assert!(producer.try_produce(test_message).is_some());

    assert_eq!(consumer.consume(), test_message);
    assert_eq!(consumer.try_consume(), None);
    assert!(
        producer.try_produce(test_message).is_some(),
        "send should fail here due to observer not having observed."
    );

    assert_eq!(observer.strong_observe(), test_message);
    assert_eq!(observer.try_strong_observe(), None);

    assert_eq!(producer.try_produce(test_message), None);
}

pub(crate) fn test_produce_weak_observe<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let producer = oqueue.attach_producer().unwrap();
    let consumer = oqueue.attach_consumer().unwrap();
    let weak_observer = oqueue.attach_weak_observer().unwrap();

    let recent_cursor = weak_observer.recent_cursor();
    assert_eq!(weak_observer.weak_observe(recent_cursor), None);

    let test_message = TestMessage { x: 42 };
    producer.produce(test_message);

    // Check recent cursor
    let recent_cursor = weak_observer.recent_cursor();
    assert_eq!(
        weak_observer.weak_observe(recent_cursor),
        Some(test_message)
    );

    // Check oldest cursor
    let oldest_cursor = weak_observer.oldest_cursor();
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor),
        Some(test_message)
    );

    assert_eq!(consumer.consume(), test_message);

    assert_eq!(weak_observer.recent_cursor(), recent_cursor);
    assert_eq!(weak_observer.oldest_cursor(), oldest_cursor);
    assert_eq!(
        weak_observer.weak_observe(recent_cursor),
        Some(test_message)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor),
        Some(test_message)
    );

    let test_message_2 = TestMessage { x: 43 };

    producer.produce(Clone::clone(&test_message_2));
    assert_eq!(consumer.consume(), test_message_2);

    let recent_cursor = weak_observer.recent_cursor();
    assert_eq!(
        weak_observer.weak_observe(recent_cursor),
        Some(test_message_2)
    );
    let oldest_cursor = weak_observer.oldest_cursor();
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor),
        Some(test_message)
    );

    let test_message_3 = TestMessage { x: 44 };

    producer.produce(test_message_3);

    assert_eq!(weak_observer.weak_observe(oldest_cursor), None);
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 1),
        Some(test_message_2)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 2),
        Some(test_message_3)
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
pub(crate) fn test_send_receive_blocker<T: OQueue<TestMessage>>(
    oqueue: Arc<T>,
    n_messages: usize,
    n_observers: usize,
) {
    let queue = Arc::new(WaitQueue::new());
    let completed_threads = Arc::new(AtomicUsize::new(0));

    // Consumer which receives all the messages
    let received_messages = Arc::new(Mutex::new(Vec::with_capacity(n_messages)));
    let received_thread = TaskOptions::new({
        let consumer = oqueue.attach_consumer().unwrap();
        let received_messages = Arc::clone(&received_messages);
        let completed_threads = completed_threads.clone();
        let queue = queue.clone();
        move || {
            for i in 0..n_messages {
                let message = consumer.consume();
                assert_eq!(message.x, i);
                received_messages.lock().push(message);
            }
            completed_threads.fetch_add(1, Ordering::Relaxed);
            queue.wake_all();
        }
    })
    .spawn()
    .unwrap();

    // Observers which strong observe all of the messages
    let observers: Vec<_> = (0..n_observers)
        .map(|i| {
            let strong_observer = oqueue.attach_strong_observer().unwrap();
            let completed_threads = completed_threads.clone();
            let queue = queue.clone();
            TaskOptions::new({
                move || {
                    for i in 0..n_messages {
                        let message = strong_observer.strong_observe();
                        assert_eq!(message.x, i);
                    }
                    completed_threads.fetch_add(1, Ordering::Relaxed);
                    queue.wake_all();
                }
            })
            .spawn()
            .unwrap();
        })
        .collect();

    // Producer thread which sends n messages
    let producer_thread = TaskOptions::new({
        let producer = oqueue.attach_producer().unwrap();
        let completed_threads = completed_threads.clone();
        let queue = queue.clone();
        move || {
            for x in 0..n_messages {
                producer.produce(TestMessage { x });
                sleep(Duration::from_millis(3));
            }
            completed_threads.fetch_add(1, Ordering::Relaxed);
            queue.wake_all();
        }
    })
    .spawn()
    .unwrap();

    // server_thread, received_thread, observers
    let n_threads = 1 + 1 + observers.len();
    queue.wait_until(|| {
        if completed_threads.load(Ordering::Relaxed) == n_threads {
            Some(())
        } else {
            None
        }
    });

    let received_messages = received_messages.lock();
    assert_eq!(received_messages.len(), n_messages);
}

/// Check that multithreading works at a basic level.
pub(crate) fn test_send_multi_receive_blocker<T: OQueue<TestMessage>>(
    oqueue1: Arc<T>,
    oqueue2: Arc<T>,
    n_messages: usize,
) {
    // Consumer which receives all the messages
    let consumer1 = oqueue1.attach_consumer().unwrap();
    let consumer2 = oqueue2.attach_consumer().unwrap();
    let recv_queue = Arc::new(WaitQueue::new());
    let recv_completed = Arc::new(AtomicBool::new(false));
    let receive_thread = TaskOptions::new({
        let recv_queue = recv_queue.clone();
        let recv_completed = recv_completed.clone();
        move || {
            let mut consumer1_counter = 0;
            let mut consumer2_counter = 0;

            while consumer1_counter < n_messages || consumer2_counter < n_messages {
                select!(
                    if let TestMessage { x } = consumer1.try_consume() {
                        assert_eq!(x, consumer1_counter);
                        consumer1_counter += 1;
                    },
                    if let TestMessage { x } = consumer2.try_consume() {
                        assert_eq!(x, consumer2_counter);
                        consumer2_counter += 1;
                    }
                )
            }
            recv_completed.store(true, Ordering::Relaxed);
            recv_queue.wake_all();
        }
    })
    .spawn()
    .unwrap();

    let producer_queue = Arc::new(WaitQueue::new());
    // Producer thread which sends n messages
    let producer_thread_completions: Vec<_> = [oqueue1, oqueue2]
        .into_iter()
        .enumerate()
        .map(|(i, oqueue)| {
            let producer = oqueue.attach_producer().unwrap();
            let completed = Arc::new(AtomicBool::new(false));
            TaskOptions::new({
                let completed = completed.clone();
                let producer_queue = producer_queue.clone();
                move || {
                    for x in 0..n_messages {
                        producer.produce(TestMessage { x });
                        sleep(Duration::from_millis(i as u64 + 1));
                    }
                    completed.store(true, Ordering::Relaxed);
                    producer_queue.wake_all();
                }
            })
            .spawn()
            .unwrap();
            completed
        })
        .collect();

    // Wait for all threads to finish
    for completed in producer_thread_completions {
        producer_queue.wait_until(|| completed.load(Ordering::Relaxed).then_some(()));
    }
    recv_queue.wait_until(|| recv_completed.load(Ordering::Relaxed).then_some(()));
}

/// Test produce operations with strong observation, but without any consumer attached. Especially,
/// if the queue blocks when there is no consumer.
pub(crate) fn test_produce_strong_observe_only(oqueue: Arc<dyn OQueue<TestMessage>>) {
    let test_message = TestMessage { x: 42 };

    let producer = oqueue.attach_producer().unwrap();
    let observer = oqueue.attach_strong_observer().unwrap();

    for i in 0..100 {
        assert_eq!(
            producer.try_produce(test_message),
            None,
            "failed to produce at iteration {i}"
        );
        assert_eq!(observer.strong_observe(), test_message);
    }
}

pub(crate) fn test_consumer_late_attach<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let producer = oqueue.attach_producer().unwrap();

    producer.produce(TestMessage { x: 1 });

    let consumer = oqueue.attach_consumer().unwrap();

    producer.produce(TestMessage { x: 2 });

    // Should only receive the second message because we attached after the first.
    assert_eq!(consumer.consume(), TestMessage { x: 2 });
    assert_eq!(consumer.try_consume(), None);
}

pub(crate) fn test_consumer_detach<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let producer = oqueue.attach_producer().unwrap();
    let consumer = oqueue.attach_consumer().unwrap();

    producer.produce(TestMessage { x: 0 });
    assert_eq!(consumer.consume(), TestMessage { x: 0 });

    // Detach the consumer.
    drop(consumer);

    // The queue should no longer be blocked.
    for i in 0..100 {
        assert_eq!(
            producer.try_produce(TestMessage { x: i }),
            None,
            "failed at {}",
            i
        );
    }
}

pub(crate) fn test_strong_observer_late_attach(oqueue: Arc<dyn OQueue<TestMessage>>) {
    let producer = oqueue.attach_producer().unwrap();

    producer.produce(TestMessage { x: 1 });

    let observer = oqueue.attach_strong_observer().unwrap();

    producer.produce(TestMessage { x: 2 });

    // Should only observe the second message because we attached after the first.
    assert_eq!(observer.strong_observe(), TestMessage { x: 2 });
    assert_eq!(observer.try_strong_observe(), None);
}

pub(crate) fn test_strong_observer_detach(oqueue: Arc<dyn OQueue<TestMessage>>) {
    let producer = oqueue.attach_producer().unwrap();
    let observer = oqueue.attach_strong_observer().unwrap();

    producer.produce(TestMessage { x: 0 });
    assert_eq!(observer.strong_observe(), TestMessage { x: 0 });

    // Detach the observer.
    drop(observer);

    // The queue should no longer be blocked.
    for i in 0..100 {
        assert_eq!(
            producer.try_produce(TestMessage { x: i }),
            None,
            "failed at {}",
            i
        );
    }
}
