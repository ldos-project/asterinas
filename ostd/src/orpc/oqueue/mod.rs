//! Observable Queues (OQueues)
//!
//! TODO: Copy docs from papers and presentation materials.
//!
//! ## Traits and types
//!
//! The OQueue interface is 3 traits:
//!
//! * [`OQueue<T>`] provides observation. This is the base trait provided by all OQueues.
//! * [`CommunicationOQueue<T: Send>`] provides produce by value and consume. Message ownership is
//!   passed from the producer to the consumer. This is used for OQueues which are communication
//!   channels between servers. The transfer of ownership allows the message to contain values that
//!   should not or can not be cloned.
//! * [`ObservationOQueue<T>`] provides produce by reference, but not consume. Message ownership is
//!   kept by the producer, so consumers cannot exist. This is used for OQueues which expose
//!   internal component state since it does not require copying the message before producing it.
//!   Only the observed parts of the message need to be copied.
//!
//! There are 4 reference types for OQueues. 3 concrete structs implement the traits above:
//! [`OQueueRef<T>`], [`CommunicationOQueueRef<T: Send>`], [`ObservationOQueueRef<T>`]. A 4th struct
//! [`AnyOQueueRef<T>`] implements both `CommunicationOQueue<T>` and `ObservationOQueue<T>`, meaning
//! it represents an OQueue of unknown type.
//!
//! Attachment operations on OQueues can fail at call time if they are not supported or are not
//! allowed. This can occur because the OQueue is of the wrong kind (e.g., a `AnyOQueueRef`
//! referencing an observation queue cannot provide a consume handle), or if a specific kind of
//! attachment is banned for safety (e.g., an observation OQueue filled by the scheduler does not
//! allow strong observers to eliminate the risk of produce blocking).
//!
//! ## Observation
//!
//! Observation occurs via [`StrongObserver<U>`] and [`WeakObserver<U>`]. These are created on a
//! specific OQueue and include a query. A query is a function which extracts a observable value of
//! type `U` from a message in the OQueue of type `T`. They are run in the producing context. The
//! observable value must be `Copy + Send` regardless of the message type. This allows the
//! observable value to be efficiently observed by multiple strong and weak observers.

// TODO: Do we need a 5th struct `UntypedOQueueRef` has no message type parameter and represents an
// unknown OQueue. It only supports casting dynamically to an `AnyOQueueRef<T>`.

use alloc::{sync::Arc, vec, vec::Vec};
use core::{
    error::Error,
    marker::PhantomData,
    ops::{Add, Sub},
};

mod inner;
pub(crate) mod query;

pub use query::ObservationQuery;

mod interface {
    use alloc::boxed::Box;

    use super::*;

    /// Errors possible for accessing an OQueue via a handle.
    #[non_exhaustive]
    #[derive(Debug)]
    pub enum HandleAccessError {
        /// The handle has been revoked. Once this is returned, all future operations will return it.
        AccessRevoked,
        // /// The handle has crashed and will no longer function. For example, the extractor panicked.
        // Failed { source: Box<dyn Error> },

        // TODO: Do we need a "closed" error for OQueues which were destroyed?
    }

    /// Error possible when attaching to an OQueue.
    #[non_exhaustive]
    #[derive(Debug)]
    pub enum AttachmentError {
        /// The operation is supported by this OQueue but the required resources are missing (e.g.,
        /// observer slots).
        ResourceUnavailable,

        /// The operation is not supported or not allowed by this OQueue. An operation may not be
        /// supported if, for example, the OQueue is an observation OQueue and a client tries to
        /// attach a consumer. An operation may not be allowed to prevent problems such as potential
        /// hangs or crashes. For example, strong observers may not be allowed on an OQueue to
        /// prevent `produce` blocking.
        Unsupported,
    }

    /// The interface provided by all OQueues.
    pub trait OQueue<T> {
        /// Attach a strong observer which will observe values of type `U` which are extracted from
        /// the messages using the query.
        fn attach_strong_observer<U>(
            &self,
            query: ObservationQuery<T, U>,
        ) -> Result<StrongObserver<U>, AttachmentError>
        where
            U: Copy + Send;

        /// Attach a weak observer which will observer values of type `U` which are extracted from
        /// the messages using the query. The history length specifies how many previous values the
        /// observer wishes to see.
        fn attach_weak_observer<U>(
            &self,
            history_len: usize,
            query: ObservationQuery<T, U>,
        ) -> Result<WeakObserver<U>, AttachmentError>
        where
            U: Copy + Send;

        /// Erase the kind of OQueue. This will not allow additional operations to succeed. It
        /// simply makes the checks dynamic.
        fn as_any_oqueue(&self) -> AnyOQueueRef<T>;
    }

    /// An OQueue for communication which support producing by value and consumers.
    pub trait CommunicationOQueue<T>: OQueue<T>
    where
        T: Send,
    {
        /// Attach a producer to the queue which will use it for communication by moving messaged
        /// into the OQueue.
        fn attach_communication_producer(
            &self,
        ) -> Result<CommunicationProducer<T>, AttachmentError>;

        fn attach_consumer(&self) -> Result<Consumer<T>, AttachmentError>;
    }

    /// An OQueue for observation which support producing by reference and does not allow consumers.
    pub trait ObservationOQueue<T>: OQueue<T> {
        /// Attach a producer to the queue which will be used to observe state, instead of communicate.
        /// OQueues used this way cannot have consumers.
        fn attach_observation_producer(&self) -> Result<ObservationProducer<T>, AttachmentError>;
    }
}

pub use interface::{
    AttachmentError, CommunicationOQueue, HandleAccessError, OQueue, ObservationOQueue,
};

macro_rules! impl_oqueue_forward {
    ($type_name:ident, $member:ident) => {
        impl<T> OQueue<T> for $type_name<T> {
            fn attach_strong_observer<U>(
                &self,
                query: ObservationQuery<T, U>,
            ) -> Result<StrongObserver<U>, AttachmentError>
            where
                U: Copy + Send,
            {
                self.$member.attach_strong_observer(query)
            }

            fn attach_weak_observer<U>(
                &self,
                history_len: usize,
                query: ObservationQuery<T, U>,
            ) -> Result<WeakObserver<U>, AttachmentError>
            where
                U: Copy + Send,
            {
                self.$member.attach_weak_observer(history_len, query)
            }

            fn as_any_oqueue(&self) -> AnyOQueueRef<T> {
                self.$member.as_any_oqueue()
            }
        }
    };
}

macro_rules! impl_communication_oqueue_forward {
    ($type_name:ident, $member:ident) => {
        impl<T: Send> CommunicationOQueue<T> for $type_name<T>
        where
            T: Send,
        {
            fn attach_communication_producer(
                &self,
            ) -> Result<CommunicationProducer<T>, AttachmentError> {
                self.$member.attach_communication_producer()
            }

            fn attach_consumer(&self) -> Result<Consumer<T>, AttachmentError> {
                self.$member.attach_consumer()
            }
        }
    };
}

macro_rules! impl_observation_oqueue_forward {
    ($type_name:ident, $member:ident) => {
        impl<T> ObservationOQueue<T> for $type_name<T> {
            fn attach_observation_producer(
                &self,
            ) -> Result<ObservationProducer<T>, AttachmentError> {
                self.$member.attach_observation_producer()
            }
        }
    };
}

/// A dynamically typed OQueue that allows attempting any kind of attachment, but dynamically
/// returns errors for unsupported ones.
pub struct AnyOQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl_oqueue_forward!(AnyOQueueRef, inner);
impl_communication_oqueue_forward!(AnyOQueueRef, inner);
impl_observation_oqueue_forward!(AnyOQueueRef, inner);

/// A reference to an OQueue of an unknown kind, meaning only observation is allowed.
#[derive(Clone)]
pub struct OQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl_oqueue_forward!(OQueueRef, inner);

/// A reference to a communication OQueue.
#[derive(Clone)]
pub struct CommunicationOQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl<T> CommunicationOQueueRef<T> {
    pub fn new(len: usize) -> Self {
        todo!()
    }
}

impl_oqueue_forward!(CommunicationOQueueRef, inner);
impl_communication_oqueue_forward!(CommunicationOQueueRef, inner);

/// A reference to an observation OQueue.
#[derive(Clone)]
pub struct ObservationOQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl<T> ObservationOQueueRef<T> {
    pub fn new() -> Self {
        todo!()
    }
}

impl_oqueue_forward!(ObservationOQueueRef, inner);
impl_observation_oqueue_forward!(ObservationOQueueRef, inner);

/// An attachment to an OQueue which allows transferring ownership of messages from the producer to
/// the consumer without copying or cloning.
pub struct CommunicationProducer<T> {
    oqueue: OQueueRef<T>,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl<T: Send> CommunicationProducer<T> {
    /// Produce a value, giving up ownership. This is used to pass an object to the consumer.
    pub fn produce(&self, v: T) {
        todo!()
    }

    /// Try to produce a value without blocking. Returns `Err(v)` if the operation would block,
    /// `None` if the value was successfully produced.
    pub fn try_produce(&self, v: T) -> Result<(), T> {
        todo!()
    }
}

/// An attachment to an OQueue which allows producing values values by reference for observation.
/// There can be no consumers since the message is not moved into the OQueue.
pub struct ObservationProducer<T> {
    oqueue: OQueueRef<T>,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl<T> ObservationProducer<T> {
    /// Produce a value for observation. The value can be taken by reference, since only observers
    /// are allowed, meaning that only values extracted from the the value need to be stored.
    pub fn produce_ref(&self, v: &T) {
        todo!()
    }

    /// Try to produce a value for observation without blocking. Returns `false` if the operation
    /// would block, `true` if the value was successfully produced.
    pub fn try_produce_ref(&self, v: &T) -> bool {
        todo!()
    }
}

pub struct Consumer<T> {
    oqueue: OQueueRef<T>,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl<T> Consumer<T> {
    /// Consume a value from the queue, taking ownership of that value.
    pub fn consume(&self) -> T {
        todo!()
    }

    /// Try to consume a value without blocking. Returns `None` if no value is available.
    pub fn try_consume(&self) -> Option<T> {
        todo!()
    }
}

pub struct StrongObserver<U: Copy + Send> {
    // XXX: How to handle erasing T?
    _phantom: PhantomData<core::cell::Cell<U>>,
}

impl<U: Copy + Send> StrongObserver<U> {
    /// Observe a value from the queue. This value will have been extracted from the message by the
    /// query provided on attachment.
    pub fn strong_observe(&self) -> Result<U, HandleAccessError> {
        todo!()
    }

    /// Try to observe a value without blocking. Returns `None` if no value is available.
    pub fn try_strong_observe(&self) -> Result<Option<U>, HandleAccessError> {
        todo!()
    }
}

/// A cursor into an OQueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor(usize);

impl Add<usize> for Cursor {
    type Output = Cursor;

    fn add(self, rhs: usize) -> Self::Output {
        Cursor(self.0 + rhs)
    }
}

impl Sub<usize> for Cursor {
    type Output = Cursor;

    fn sub(self, rhs: usize) -> Self::Output {
        Cursor(self.0 - rhs)
    }
}

pub struct WeakObserver<U: Copy + Send> {
    // XXX: How to handle erasing T?
    _phantom: PhantomData<core::cell::Cell<U>>,
}

impl<U: Copy + Send> WeakObserver<U> {
    /// Wait until new values are available (that is values that have not yet been observed). This
    /// does not guarantee that the caller will be able to observe those specific values. Only, that
    /// the caller will run eventually when new values are produced.
    pub fn wait(&self) {
        todo!()
    }

    // Low-level interface:

    pub fn recent_cursor(&self) -> Cursor {
        todo!()
    }
    pub fn old_cursor(&self) -> Cursor {
        todo!()
    }

    pub fn weak_observe(&self, i: Cursor) -> Result<Option<U>, HandleAccessError> {
        todo!()
    }

    // High-level interface

    /// Observe the most recent `n` values. Any values that cannot be accessed (due to the history
    /// being too short or them being concurrently overwritten) will be replaced by `None`. The
    /// returned `Vec` always has length `n`.
    pub fn weak_observe_recent(&self, n: usize) -> Result<Vec<Option<U>>, HandleAccessError> {
        let mut ret: Vec<_> = vec![None; n];
        self.weak_observe_recent_into(&mut ret)?;
        Ok(ret)
    }

    /// Observe the most recent `buf.len()` values. Any values that cannot be accessed (due to the
    /// history being too short or them being concurrently overwritten) will be replaced by `None`.
    pub fn weak_observe_recent_into(&self, buf: &mut [Option<U>]) -> Result<(), HandleAccessError> {
        let recent = self.recent_cursor();
        for i in 0..buf.len() {
            buf[i] = self.weak_observe(recent - (buf.len() - i - 1))?;
        }
        Ok(())
    }
}
