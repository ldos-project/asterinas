mod generic_test;
pub mod locking;
pub mod registry;

use core::ops::{Add, Sub};
use std::{
    any::Any,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::Arc,
};

use snafu::Snafu;

use crate::sync::blocker::Blocker;

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

/// A sender handle to a queue. This allow putting values into the queue. Senders are also called senders.
pub trait Sender<T>: Send + UnwindSafe + Blocker {
    /// Send a message. This is also called producing a value.
    fn send(&self, data: T);

    /// Send a message (a.k.a. produce a value) if there is space, otherwise return it to the caller. If this returns
    /// `None`, the put succeeded.
    fn try_send(&self, data: T) -> Option<T>;
}

static_assertions::assert_obj_safe!(Sender<usize>);

/// A receiver handle to a oqueue. This allows taking or receiving values from the oqueue such that no other receiver will
/// receive the same value ("exactly once to exactly one" semantics).
pub trait Receiver<T>: Send + UnwindSafe + Blocker {
    /// Receive a message. This is also called consuming a value.
    ///
    /// This has "exactly once to exactly one receiver" semantics.
    fn receive(&self) -> T;

    /// Receive a message (a.k.a. consume a value) if there is something in the queue.
    fn try_receive(&self) -> Option<T>;

    // fn receive_async(&self) -> ReceiverPromise<T> {
    //     ReceiverPromise(self)
    // }
}

static_assertions::assert_obj_safe!(Receiver<()>);

/// A strong-observer handle to a oqueue. This allows receiving every value from a oqueue without preventing other
/// receivers or observers from seeing the same value ("exactly once to each" semantics). If a strong observer falls
/// behind on observing elements it will cause the oqueue to block senders, so strong observers must make sure they
/// process data promptly.
pub trait StrongObserver<T>: Send + UnwindSafe + Blocker {
    /// Observe some data. The caller must be subscribed as a strict observer.
    ///
    /// This has "exactly once to each observer" semantics.
    fn strong_observe(&self) -> T;

    /// Observe an element from the oqueue if it is immediately available.
    fn try_strong_observe(&self) -> Option<T>;
}

/// A weak-observer handle to a oqueue. This allows looking at the history of the oqueue without affecting any other
/// senders, receivers, or observers. Weak-observers are not guaranteed to observe every element, so they never block
/// senders (which can simply overwrite data). However, weak-observers are guaranteed to alway get either nothing or
/// the data at the cursor requested.
///
/// When used as a blocker this will wake if there is unobserved data in the queue. Code using this should make sure
/// they always read up to the most recent value before attempting to block. In the simplest case this is:
/// `observer.weak_observe(self.recent_cursor())`.
pub trait WeakObserver<T>: Send + UnwindSafe + Blocker {
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
/// messages passing through. If no receivers are attached, messages will just disappear once they are observed. If no
/// receivers and no observers are attached, then messages will be written into the buffer whenever they are produced.
/// Weak observers can still observe the messages.
///
/// NOTE: Due to the needs to ORPC, OQueues should generally implement `RefUnwindSafe`.
pub trait OQueue<T>: Any + Sync + Send + RefUnwindSafe {
    /// Attach to the oqueue as a sender. An error represents either that senders are not supported or that senders
    /// are supported but all supported senders are already attached (for instance, if a second sender tries to
    /// attach to a single-sender oqueue implementation).
    fn attach_sender(&self) -> Result<Box<dyn Sender<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a receiver. An error represents either that senders are not supported or that no more
    /// receivers are allowed on this specific oqueue (for example, for a single-receiver oqueue implementation).
    fn attach_receiver(&self) -> Result<Box<dyn Receiver<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a strong observer. An error represents either that strong observers are not supported or that no more
    /// strong-observers are allowed on this specific oqueue (for example, if the oqueue as a limited number of strong-observer slots).
    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<T>>, OQueueAttachError>;
    /// Attach to the oqueue as a weak-observer. An error represents either that weak-observer are not supported or that
    /// no more weak-observer are allowed on this specific oqueue (for example, if there are a limited number of
    /// weak-observer slots on the oqueue.).
    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<T>>, OQueueAttachError>;
}

/// A reference to an OQueue. This must be cloned when a new reference is needed. It is `Send`, but not `Sync`. (It
/// behaves similarly to `Arc` and as of writing is implemented as `Arc`.)
pub type OQueueRef<T> = Arc<dyn OQueue<T>>;

// TODO: The `RefUnwindSafe` requirement is questionable here. We need it for panic handling, but it's not clear how we
// guarantee that OQueues actually have the correct property. This may require that OQueues implement some form of
// poisoning (like `Mutex`). However, it's not clear what needs to be poisoned and which queues need that treatment. The
// access handles may also need unwind safety.
