#![cfg(test)]
#![allow(missing_docs)]
#![allow(unused)]

use std::{
    thread::{self, sleep},
    time::Duration,
};

use alloc::sync::Arc;

use crate::sync::{Mutex, blocker::select, task::Task};

use super::*;

#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub(crate) struct TestMessage {
    x: usize,
}

pub(crate) fn test_produce_consume<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let sender = oqueue.attach_sender().unwrap();
    let receiver = oqueue.attach_receiver().unwrap();
    let test_message = TestMessage { x: 42 };

    sender.send(test_message);
    assert!(sender.try_send(test_message).is_some());

    assert_eq!(receiver.receive(), test_message);
    assert_eq!(receiver.try_receive(), None);

    assert_eq!(sender.try_send(test_message), None);
}

pub(crate) fn test_produce_strong_observe(oqueue: Arc<dyn OQueue<TestMessage>>) {
    let sender = oqueue.attach_sender().unwrap();
    let receiver = oqueue.attach_receiver().unwrap();
    let test_message = TestMessage { x: 42 };

    // Normal operation when there is no observer
    sender.send(test_message);
    assert!(sender.try_send(test_message).is_some());

    assert_eq!(receiver.receive(), test_message);
    assert_eq!(receiver.try_receive(), None);

    assert_eq!(sender.try_send(test_message), None);
    assert_eq!(receiver.receive(), test_message);

    assert_eq!(receiver.try_receive(), None);

    // With observer we should block sooner.
    let observer = oqueue.attach_strong_observer().unwrap();

    sender.send(test_message);
    assert!(sender.try_send(test_message).is_some());

    assert_eq!(receiver.receive(), test_message);
    assert_eq!(receiver.try_receive(), None);
    assert!(
        sender.try_send(test_message).is_some(),
        "send should fail here due to observer not having observed."
    );

    assert_eq!(observer.strong_observe(), test_message);
    assert_eq!(observer.try_strong_observe(), None);

    assert_eq!(sender.try_send(test_message), None);
}

pub(crate) fn test_produce_weak_observe<T: OQueue<TestMessage>>(oqueue: Arc<T>) {
    let sender = oqueue.attach_sender().unwrap();
    let receiver = oqueue.attach_receiver().unwrap();
    let weak_observer = oqueue.attach_weak_observer().unwrap();

    let recent_cursor = weak_observer.recent_cursor();
    assert_eq!(weak_observer.weak_observe(recent_cursor), None);

    let test_message = TestMessage { x: 42 };
    sender.send(test_message);

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

    assert_eq!(receiver.receive(), test_message);

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

    sender.send(Clone::clone(&test_message_2));
    assert_eq!(receiver.receive(), test_message_2);

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

    sender.send(test_message_3);

    assert_eq!(weak_observer.weak_observe(oldest_cursor), None);
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 1),
        Some(test_message_2)
    );
    assert_eq!(
        weak_observer.weak_observe(oldest_cursor + 2),
        Some(test_message_3)
    );

    assert_eq!(receiver.receive(), test_message_3);
}

/// Check that multithreading works at a basic level.
pub(crate) fn test_send_receive_blocker<T: OQueue<TestMessage>>(
    oqueue: Arc<T>,
    n_messages: usize,
    n_observers: usize,
) {
    // Receiver which receives all the messages
    let received_messages = Arc::new(Mutex::new(Vec::with_capacity(n_messages)));
    let received_thread = thread::spawn({
        let receiver = oqueue.attach_receiver().unwrap();
        let received_messages = Arc::clone(&received_messages);
        move || {
            for i in 0..n_messages {
                let message = receiver.receive();
                assert_eq!(message.x, i);
                received_messages.lock().push(message);
            }
        }
    });

    // Observers which strong observe all of the messages
    let observers: Vec<_> = (0..n_observers)
        .map(|_| {
            let strong_observer = oqueue.attach_strong_observer().unwrap();
            thread::spawn({
                move || {
                    for i in 0..n_messages {
                        let message = strong_observer.strong_observe();
                        assert_eq!(message.x, i);
                    }
                }
            })
        })
        .collect();

    // Sender thread which sends n messages
    let sender_thread = thread::spawn({
        let sender = oqueue.attach_sender().unwrap();
        move || {
            for x in 0..n_messages {
                sender.send(TestMessage { x });
                sleep(Duration::from_millis(3));
            }
        }
    });

    // Wait for all threads to finish
    sender_thread.join().unwrap();
    received_thread.join().unwrap();
    for observer in observers {
        observer.join().unwrap();
    }

    let received_messages = received_messages.lock();
    assert_eq!(received_messages.len(), n_messages);
}

/// Check that multithreading works at a basic level.
pub(crate) fn test_send_multi_receive_blocker<T: OQueue<TestMessage>>(
    oqueue1: Arc<T>,
    oqueue2: Arc<T>,
    n_messages: usize,
) {
    // Receiver which receives all the messages
    let receiver1 = oqueue1.attach_receiver().unwrap();
    let receiver2 = oqueue2.attach_receiver().unwrap();
    let receive_thread = thread::spawn({
        move || {
            let mut receiver1_counter = 0;
            let mut receiver2_counter = 0;

            while receiver1_counter < n_messages || receiver2_counter < n_messages {
                select!(
                    if let TestMessage { x } = receiver1.try_receive() {
                        assert_eq!(x, receiver1_counter);
                        receiver1_counter += 1;
                    },
                    if let TestMessage { x } = receiver2.try_receive() {
                        assert_eq!(x, receiver2_counter);
                        receiver2_counter += 1;
                    }
                )
            }
        }
    });

    // Sender thread which sends n messages
    let sender_threads: Vec<_> = [oqueue1, oqueue2]
        .into_iter()
        .enumerate()
        .map(|(i, oqueue)| {
            let sender = oqueue.attach_sender().unwrap();
            thread::spawn(move || {
                for x in 0..n_messages {
                    sender.send(TestMessage { x });
                    sleep(Duration::from_millis(i as u64 + 1));
                }
            })
        })
        .collect();

    // Wait for all threads to finish
    for t in sender_threads {
        t.join().unwrap();
    }
    receive_thread.join().unwrap();
}
