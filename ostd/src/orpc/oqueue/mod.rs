use alloc::{sync::Arc, vec::Vec};
use core::{
    error::Error, marker::PhantomData, ops::{Add, Sub}
};

mod inner;

mod interface {
    use super::*;

    /// The interface provided by all OQueues.
    pub trait OQueue<T> {
        fn attach_strong_observer<U>(
            &self,
            extractor: impl Fn(&T) -> U,
        ) -> Result<StrongObserver<U>, AttachmentError>
        where
            U: Copy + Send;

        /// Attach a strong observer to the OQueue by calling a strong observer function with each value.
        /// 
        /// Note: This is different from ``
        fn attach_inline_strong_observer(
            &self,
            f: impl Fn(&T) + Sync + Send + 'static,
        ) -> Result<(), AttachmentError>;

        fn attach_weak_observer<U>(
            &self,
            history_len: usize,
            extractor: impl Fn(&T) -> U,
        ) -> Result<WeakObserver<U>, AttachmentError>
        where
            U: Copy + Send;
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
        fn attach_observation_producer(&self) -> ObservationProducer<T>;
    }
}

pub use interface::{CommunicationOQueue, OQueue, ObservationOQueue};

/// Errors possible for accessing an OQueue via a handle.
#[non_exhaustive]
#[derive(Debug)]
pub enum HandleAccessError {
    /// The handle has been revoked. Once this is returned, all future operations will return it.
    AccessRevoked,
    // TODO: Do we need a "closed" error for OQueues which were destroyed?
}

/// Error possible when attaching to an OQueue.
#[non_exhaustive]
#[derive(Debug)]
pub enum AttachmentError {
    /// The operation is supported by this OQueue but the required resources are missing (e.g.,
    /// observer slots).
    ResourceUnavailable,

    /// The operation is not supported by this OQueue.
    Unsupported,
}

/// A reference to an OQueue of an unknown kind, meaning only observation is allowed.
#[derive(Clone)]
pub struct OQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl<T> OQueue<T> for OQueueRef<T> {
    fn attach_strong_observer<U>(
        &self,
        extractor: impl Fn(&T) -> U,
    ) -> Result<StrongObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }

    fn attach_weak_observer<U>(
        &self,
        history_len: usize,
        extractor: impl Fn(&T) -> U,
    ) -> Result<WeakObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }
}

/// A reference to a communication OQueue.
#[derive(Clone)]
pub struct CommunicationOQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl<T: Send> CommunicationOQueue<T> for CommunicationOQueueRef<T> {
    fn attach_communication_producer(&self) -> Result<CommunicationProducer<T>, AttachmentError> {
        todo!()
    }

    fn attach_consumer(&self) -> Result<Consumer<T>, AttachmentError> {
        todo!()
    }
}

impl<T> OQueue<T> for CommunicationOQueueRef<T> {
    fn attach_strong_observer<U>(
        &self,
        extractor: impl Fn(&T) -> U,
    ) -> Result<StrongObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }

    fn attach_weak_observer<U>(
        &self,
        history_len: usize,
        extractor: impl Fn(&T) -> U,
    ) -> Result<WeakObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }
}

/// A reference to an observation OQueue.
#[derive(Clone)]
pub struct ObservationOQueueRef<T> {
    inner: Arc<inner::OQueueInner<T>>,
}

impl<T> ObservationOQueue<T> for ObservationOQueueRef<T> {
    fn attach_observation_producer(&self) -> ObservationProducer<T> {
        todo!()
    }
}

impl<T> OQueue<T> for ObservationOQueueRef<T> {
    fn attach_strong_observer<U>(
        &self,
        extractor: impl Fn(&T) -> U,
    ) -> Result<StrongObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }

    fn attach_weak_observer<U>(
        &self,
        history_len: usize,
        extractor: impl Fn(&T) -> U,
    ) -> Result<WeakObserver<U>, AttachmentError>
    where
        U: Copy + Send,
    {
        todo!()
    }
}

// XXX: It is possible to adapt between the two types of produce for `T: Clone` or simply a
// discarded value in their respective cases.

/// An attachment to an OQueue which allows transferring ownership of messages from the producer to
/// the consumer without copying or cloning.
pub struct CommunicationProducer<T> {
    oqueue: OQueueRef<T>,
}

impl<T: Send> CommunicationProducer<T> {
    pub fn produce(&self, v: T) {
        todo!()
    }
}

/// An attachment to an OQueue which allows producing values values by reference for observation.
/// There can be no consumers since the message is not moved into the OQueue.
pub struct ObservationProducer<T> {
    oqueue: OQueueRef<T>,
}

impl<T> ObservationProducer<T> {
    /// Produce a value for observation. The value can be taken by reference, since only observers
    /// are allowed, meaning that only values extracted from the the value need to be stored.
    pub fn produce_ref(&self, v: &T) {
        todo!()
    }
}

pub struct Consumer<T> {
    oqueue: OQueueRef<T>,
}

impl<T> Consumer<T> {
    pub fn consume(&self) -> T {
        todo!()
    } 

    pub fn consume_inline<E: Error>(self, f: FnMut(T) -> Result<(), E>) {
    }
}

pub struct StrongObserver<U: Copy + Send> {
    // XXX: How to handle erasing T?
    _phantom: PhantomData<[U]>,
}

impl<U: Copy + Send> StrongObserver<U> {
    pub fn strong_observe(&self) -> Result<U, HandleAccessError> {
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
    _phantom: PhantomData<[U]>,
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

    /// High-level interface

    /// Observe the most recent `n` values.Any values that cannot be accessed (due to the history
    /// being history being too short or them being concurrently overwritten) will be replaced by
    /// `None`. The returned `Vec` always has length `n`.
    pub fn weak_observe_recent(&self, n: usize) -> Result<Vec<Option<U>>, HandleAccessError> {
        let mut ret = Vec::with_capacity(n); // TODO: Need to actually create the Nones
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
