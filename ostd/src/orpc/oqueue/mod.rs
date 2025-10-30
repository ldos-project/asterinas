// SPDX-License-Identifier: MPL-2.0
//! OQueue - Observable Queue
//! OQueue provides an interface for passing data within or between subsystems in a way that can be
//! Observed by policies.

#[cfg(ktest)]
pub mod generic_test;

pub mod locking;
pub mod registry;
pub mod reply;

use alloc::string::String;
use core::{
    any::Any,
    ops::{Add, Sub},
};

use snafu::Snafu;

use super::sync::Blocker;
use crate::prelude::{Arc, Box};

/// A reference to a specific row in a queue. This refers to an element over the full history of a oqueue, not based on
/// some implementation defined buffer.
///
/// See [`WeakObserver`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor(usize);

impl Cursor {
    /// Get the global index of the cursor. I.e., the index of the row the cursor points to in a hypothetical infinite
    /// buffer containing all rows ever added to the oqueue.
    pub fn index(&self) -> usize {
        self.0
    }
}

impl Add<usize> for Cursor {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        let Cursor(i) = self;
        Cursor(i.checked_add(rhs).unwrap())
    }
}

impl Sub<usize> for Cursor {
    type Output = Self;
    fn sub(self, rhs: usize) -> Self::Output {
        let Cursor(i) = self;
        Cursor(i.saturating_sub(rhs))
    }
}

/// A producer handle to a queue. This allow putting values into the queue. Producers are also called producers.
pub trait Producer<T>: Send + Blocker {
    /// Produce a value. This sends `data` to a `Consumer` and makes it available for observation.
    fn produce(&self, data: T);

    /// Produce a value (a.k.a. send a message) if there is space, otherwise return it to the caller. If this returns
    /// `None`, the put succeeded.
    fn try_produce(&self, data: T) -> Option<T>;
}

/// A consumer handle to a oqueue. This allows taking or receiving values from the oqueue such that no other consumer will
/// receive the same value ("exactly once to exactly one" semantics).
pub trait Consumer<T>: Send + Blocker {
    /// Consume a value. This is also called receiving a message.
    ///
    /// This has "exactly once to exactly one consumer" semantics.
    fn consume(&self) -> T;

    /// Consume a value (a.k.a. receive a message) if there is something in the queue.
    fn try_consume(&self) -> Option<T>;
}

/// A strong-observer handle to a oqueue. This allows receiving every value from a oqueue without preventing other
/// consumers or observers from seeing the same value ("exactly once to each" semantics). If a strong observer falls
/// behind on observing elements it will cause the oqueue to block producers, so strong observers must make sure they
/// process data promptly.
pub trait StrongObserver<T>: Send + Blocker {
    /// Observe some data. The caller must be subscribed as a strict observer.
    ///
    /// This has "exactly once to each observer" semantics.
    fn strong_observe(&self) -> T;

    /// Observe an element from the oqueue if it is immediately available.
    fn try_strong_observe(&self) -> Option<T>;
}

/// A weak-observer handle to a oqueue. This allows looking at the history of the oqueue without affecting any other
/// producers, consumers, or observers. Weak-observers are not guaranteed to observe every element, so they never block
/// producers (which can simply overwrite data). However, weak-observers are guaranteed to alway get either nothing or
/// the data at the cursor requested.
///
/// When used as a blocker this will wake if there is unobserved data in the queue. Code using this should make sure
/// they always read up to the most recent value before attempting to block. In the simplest case this is:
/// `observer.weak_observe(self.recent_cursor())`.
pub trait WeakObserver<T>: Send + Blocker {
    /// Observe the data at the given index in the full history of the oqueue. If the data has already been discarded
    /// this will return `None`. This is guaranteed to always return either `None` or the actual value that existed at
    /// the given index.
    fn weak_observe(&self, index: Cursor) -> Option<T>;

    /// Wait for new data to become available.
    fn wait(&self);

    /// Return a cursor pointing to the most recent value in the oqueue. This has very relaxed consistency, the element
    /// may no longer be the most recent or even no longer be available.
    fn recent_cursor(&self) -> Cursor;

    /// Return a cursor pointing to the oldest value still in the oqueue. This has very relaxed consistency, the element
    /// may no longer be the oldest or even no longer be available.
    fn oldest_cursor(&self) -> Cursor;

    /// Return all available values in the range provided.
    fn weak_observe_range(&self, start: Cursor, end: Cursor) -> alloc::vec::Vec<T> {
        let mut res = alloc::vec::Vec::default();
        for i in start.index()..end.index() {
            if let Some(v) = self.weak_observe(Cursor(i)) {
                res.push(v);
            }
        }
        res
    }

    /// Return the most recent `n` values from the OQueue. Some values may be missing.
    fn weak_observe_recent(&self, n: usize) -> alloc::vec::Vec<T> {
        let now = self.recent_cursor();
        self.weak_observe_range(now - n, now)
    }
}

/// An error for attaching a handle to a [`OQueue`].
#[derive(Debug, Snafu)]
pub enum OQueueAttachError {
    /// The type of oqueue doesn't support attachment of this type.
    #[snafu(display("oqueue of type {table_type} does not support this kind of attachment"))]
    Unsupported {
        /// The name of the type which does not support the attachment.
        table_type: String,
    },

    /// An attachment slot of the given kind could not be allocated.
    #[snafu(display(
        "oqueue of type {table_type} could not allocate attachment to oqueue, because {message}"
    ))]
    AllocationFailed {
        /// The name of the type which does not support the attachment.
        table_type: String,
        /// The reason the allocation failed, for example, "not enough allocated weak-observer slots".
        message: String,
    },
}

/// An Observable Queue. The `attach_*` methods allow a user to use the queue as as a message channel or observe
/// messages passing through. If no consumers are attached, messages will just disappear once they are observed. If no
/// consumers and no observers are attached, then messages will be written into the buffer whenever they are produced.
/// Weak observers can still observe the messages.
///
/// NOTE: Due to the needs to ORPC, OQueues should generally implement `RefUnwindSafe`.
pub trait OQueue<T>: Any + Sync + Send {
    /// Attach to the oqueue as a producer. An error represents either that producers are not supported or that producers
    /// are supported but all supported producers are already attached (for instance, if a second producer tries to
    /// attach to a single-producer oqueue implementation).
    fn attach_producer(&self) -> Result<Box<dyn Producer<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a consumer. An error represents either that producers are not supported or that no more
    /// consumers are allowed on this specific oqueue (for example, for a single-consumer oqueue implementation).
    fn attach_consumer(&self) -> Result<Box<dyn Consumer<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a strong observer. An error represents either that strong observers are not supported or that no more
    /// strong-observers are allowed on this specific oqueue (for example, if the oqueue as a limited number of strong-observer slots).
    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a weak-observer. An error represents either that weak-observer are not supported or that
    /// no more weak-observer are allowed on this specific oqueue (for example, if there are a limited number of
    /// weak-observer slots on the oqueue.).
    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<T>>, OQueueAttachError>;

    /// Produce a value into the OQueue directly without attaching. This is equivalent to attaching
    /// then producing:
    /// ```ignore
    /// self.attach_producer()?.produce(v)
    /// ```
    ///
    /// By default, this will actually create an ephemeral attachment, but some OQueues will
    /// optimize it.
    fn produce(&self, v: T) -> Result<(), OQueueAttachError> {
        let producer = self.attach_producer()?;
        producer.produce(v);
        Ok(())
    }

    /// Try to produce a value into the OQueue directly without attaching. This is equivalent to
    /// attaching then trying to produce:
    /// ```ignore
    /// self.attach_producer()?.try_produce(v)
    /// ```
    ///
    /// By default, this will actually create an ephemeral attachment, but some OQueues will
    /// optimize it.
    fn try_produce(&self, v: T) -> Result<Option<T>, OQueueAttachError> {
        let producer = self.attach_producer()?;
        Ok(producer.try_produce(v))
    }
}

/// A reference to an OQueue. This must be cloned when a new reference is needed. It is `Send`, but not `Sync`. (It
/// behaves similarly to `Arc` and as of writing is implemented as `Arc`.)
pub type OQueueRef<T> = Arc<dyn OQueue<T>>;
