#![cfg(ktest)]
#![allow(missing_docs)]

use alloc::sync::Arc;

use crate::tables::{Consumer, Producer, StrongObserver, Table, WeakObserver};

#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub(crate) struct TestMessage {
    x: u64,
}

#[allow(unused)]
pub(crate) fn test_produce_consume<T: Table<TestMessage>>(table: Arc<T>) {
    let producer = table.attach_producer().unwrap();
    let consumer = table.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    producer.put(test_message.clone());
    assert!(producer.try_put(test_message.clone()).is_some());

    assert_eq!(consumer.take(), test_message);
    assert_eq!(consumer.try_take(), None);

    assert_eq!(producer.try_put(test_message.clone()), None);
}

#[allow(unused)]
pub(crate) fn test_produce_strong_observe<T: Table<TestMessage>>(table: Arc<T>) {
    let producer = table.attach_producer().unwrap();
    let consumer = table.attach_consumer().unwrap();
    let test_message = TestMessage { x: 42 };

    // Normal operation when there is no observer
    producer.put(test_message.clone());
    assert!(producer.try_put(test_message.clone()).is_some());

    assert_eq!(consumer.take(), test_message);
    assert_eq!(consumer.try_take(), None);

    assert_eq!(producer.try_put(test_message.clone()), None);
    assert_eq!(consumer.take(), test_message);

    assert_eq!(consumer.try_take(), None);

    // With observer we should block sooner.
    let observer = table.attach_strong_observer().unwrap();

    producer.put(test_message.clone());
    assert!(producer.try_put(test_message.clone()).is_some());

    assert_eq!(consumer.take(), test_message);
    assert_eq!(consumer.try_take(), None);
    assert!(
        producer.try_put(test_message.clone()).is_some(),
        "Put should fail here due to observer not having observed."
    );

    assert_eq!(observer.strong_observe(), test_message);
    assert_eq!(observer.try_strong_observe(), None);

    assert_eq!(producer.try_put(test_message.clone()), None);
}

#[allow(unused)]
pub(crate) fn test_produce_weak_observe<T: Table<TestMessage>>(table: Arc<T>) {
    let producer = table.attach_producer().unwrap();
    let consumer = table.attach_consumer().unwrap();
    let weak_observer = table.attach_weak_observer().unwrap();

    let recent_cursor = weak_observer.recent_cursor();
    assert_eq!(weak_observer.weak_observe(recent_cursor), None);

    let test_message = TestMessage { x: 42 };
    producer.put(test_message.clone());

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

    assert_eq!(consumer.take(), test_message);

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

    producer.put(test_message_2.clone());
    assert_eq!(consumer.take(), test_message_2);

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

    producer.put(test_message_3.clone());

    assert_eq!(weak_observer.weak_observe(oldest_cursor), None);
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 1),
        Some(test_message_2)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 2),
        Some(test_message_3)
    );

    assert_eq!(consumer.take(), test_message_3);
}
