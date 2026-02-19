// SPDX-License-Identifier: MPL-2.0

//! Observable Queues (OQueues)
//!
//! TODO: Copy docs from papers and presentation materials.
//!
//! ## Traits and types
//!
//! The OQueue interface is 3 traits:
//!
//! * [`OQueueBase<T>`] provides observation. This is the base trait provided by all OQueues.
//! * [`ConsumableOQueue<T: Send>`] provides produce by value and consume. Message ownership is
//!   passed from the producer to the consumer. This is used for OQueues which are communication
//!   channels between servers. The transfer of ownership allows the message to contain values that
//!   should not or can not be cloned.
//! * [`OQueue<T>`] provides produce by reference, but not consume. Message ownership is kept by the
//!   producer, so consumers cannot exist. This is used for OQueues which expose internal component
//!   state since it does not require copying the message before producing it. Only the observed
//!   parts of the message need to be copied.
//!
//! There are 4 reference types for OQueues. 3 concrete structs implement the traits above:
//! [`OQueueBaseRef<T>`], [`ConsumableOQueueRef<T: Send>`], [`OQueueRef<T>`]. A 4th struct
//! [`AnyOQueueRef<T>`] implements both `ConsumableOQueueRef<T>` and `OQueueRef<T>`. It
//! represents an OQueue of unknown type.
//!
//! Attachment operations on OQueues can fail at call time if they are not supported or are not
//! allowed. This can occur because the OQueue is of the wrong kind (e.g., a `AnyOQueueRef`
//! referencing an observation queue cannot provide a consume handle), or if a specific kind of
//! attachment is banned for safety (e.g., an OQueue filled by the scheduler does not allow strong
//! observers to eliminate the risk of produce blocking).
//!
//! ## Observation
//!
//! Observation occurs via [`StrongObserver<U>`] and [`WeakObserver<U>`]. These are created on a
//! specific OQueue and include a query. A query is a function which extracts a observable value of
//! type `U` from a message in the OQueue of type `T`. They are run in the producing context. The
//! observable value must be `Copy + Send` regardless of the message type. This allows the
//! observable value to be efficiently observed by multiple strong and weak observers.
//!
//! ## Implementation
//!
//! The types in this module are mostly wrappers around a single underlying implementation type (see
//! [`OQueueRef::inner`], for example). The wrappers carry additional information required to
//! correctly and safely use the underlying implementation. They also provide abstractions which
//! match the conceptual model of OQueues. Having this layer of wrappers will also simplify any
//! future changes to the implementation.

// TODO: Do we need a 5th struct `UntypedOQueueRef` has no message type parameter and represents an
// unknown OQueue. It only supports casting dynamically to an `AnyOQueueRef<T>`.

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::{
    any::TypeId,
    cell::Cell,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Add, Sub},
};

mod implementation;
pub mod query;
mod single_thread_ring_buffer;
mod utils;

use ostd_macros::ostd_error;
pub use query::ObservationQuery;
use snafu::Snafu;
pub use utils::new_reply_pair;

use self::implementation::{InlineObserverKey, ObserverKey};
use crate::{
    orpc::sync::Blocker,
    sync::{SpinLock, WakerKey},
};

#[cfg(ktest)]
pub(crate) mod generic_test;

/// Errors which can occur while observing an OQueue.
#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum ObservationError {
    /// The handle has been detached. Once this is returned, all future operations will return
    /// it. This can happen because access has been revoked (for instance, due to the observer
    /// being too slow), or the OQueue has been deleted.
    Detached,
}

/// Error which can occur when attaching to an OQueue.
#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum AttachmentError {
    /// The operation is supported by this OQueue but the required resources are missing (e.g.,
    /// observer slots or memory).
    ResourceUnavailable,

    /// The operation is not supported or not allowed by this OQueue. An operation may not be
    /// supported if, for example, the OQueue does not support consumers and a client tries to
    /// attach one. An operation may also be disallowed to prevent problems such as potential hangs
    /// or crashes. For example, strong observers may not be allowed on an OQueue to prevent
    /// `produce` from blocking.
    Unsupported,
}

/// The interface provided by all OQueues.
pub trait OQueueBase<T: ?Sized> {
    /// Attach a strong observer which will observe values of type `U` which are extracted from
    /// the messages using the query.
    fn attach_strong_observer<U>(
        &self,
        query: ObservationQuery<T, U>,
    ) -> Result<StrongObserver<U>, AttachmentError>
    where
        U: Copy + Send + 'static;

    /// Attach a strong observer to the OQueue by calling a strong observer function with each
    /// value.
    ///
    /// Note: This is different from [`StrongObserver::strong_observe_inline`] because this
    /// observes the entire message without requiring a filter.
    fn attach_inline_strong_observer(
        &self,
        f: impl Fn(&T) + Send + 'static,
    ) -> Result<InlineStrongObserver, AttachmentError>;

    /// Attach a weak observer which will observer values of type `U` which are extracted from
    /// the messages using the query. The history length specifies how many previous values the
    /// observer wishes to see.
    fn attach_weak_observer<U>(
        &self,
        history_len: usize,
        query: ObservationQuery<T, U>,
    ) -> Result<WeakObserver<U>, AttachmentError>
    where
        U: Copy + Send + 'static;

    /// Erase the kind of OQueue. This will not allow additional operations to succeed. It
    /// simply makes the checks dynamic.
    fn as_any_oqueue(&self) -> AnyOQueueRef<T>;
}

/// An OQueue for communication which support producing by value and consumers.
pub trait ConsumableOQueue<T>: OQueueBase<T>
where
    T: Send,
{
    /// Attach a producer to the queue which will use it for communication by moving messaged
    /// into the OQueue.
    fn attach_value_producer(&self) -> Result<ValueProducer<T>, AttachmentError>;

    /// Attach a consumer to the queue which will receive ownership of each message that is
    /// produced.
    fn attach_consumer(&self) -> Result<Consumer<T>, AttachmentError>;
}

/// An OQueue for observation which support producing by reference and does not allow consumers.
pub trait OQueue<T: ?Sized>: OQueueBase<T> {
    /// Attach a producer to the queue which will be used to observe state, instead of communicate.
    /// OQueues used this way cannot have consumers.
    fn attach_ref_producer(&self) -> Result<RefProducer<T>, AttachmentError>;
}

/// Generate an impl which forwards the OQueueBase trait to a member.
macro_rules! impl_oqueue_base_forward {
    ($type_name:ident, $member:ident, [$($added_bounds:tt)*]) => {
        impl<T: Send + 'static $($added_bounds)*> OQueueBase<T> for $type_name<T> {
            fn attach_strong_observer<U>(
                &self,
                query: ObservationQuery<T, U>,
            ) -> Result<StrongObserver<U>, AttachmentError>
            where
                U: Copy + Send + 'static,
            {
                self.$member.attach_strong_observer(query)
            }

            fn attach_weak_observer<U>(
                &self,
                history_len: usize,
                query: ObservationQuery<T, U>,
            ) -> Result<WeakObserver<U>, AttachmentError>
            where
                U: Copy + Send + 'static,
            {
                self.$member.attach_weak_observer(history_len, query)
            }

            fn attach_inline_strong_observer(
                &self,
                f: impl Fn(&T) + Send + 'static,
            ) -> Result<InlineStrongObserver, AttachmentError>
            {
                self.$member.attach_inline_strong_observer(f)
            }

            fn as_any_oqueue(&self) -> AnyOQueueRef<T> {
                self.$member.as_any_oqueue()
            }
        }
    };
}

/// Generate an impl which forwards the ConsumableOQueue trait to a member.
macro_rules! impl_consumable_oqueue_forward {
    ($type_name:ident, $member:ident) => {
        impl<T: Send + 'static> ConsumableOQueue<T> for $type_name<T> {
            fn attach_value_producer(&self) -> Result<ValueProducer<T>, AttachmentError> {
                self.$member.attach_value_producer()
            }

            fn attach_consumer(&self) -> Result<Consumer<T>, AttachmentError> {
                self.$member.attach_consumer()
            }
        }
    };
}

/// Generate an impl which forwards the OQueue trait to a member.
macro_rules! impl_oqueue_forward {
    ($type_name:ident, $member:ident, [$($added_bounds:tt)*]) => {
        impl<T: Send + 'static $($added_bounds)*> OQueue<T> for $type_name<T> {
            fn attach_ref_producer(
                &self,
            ) -> Result<RefProducer<T>, AttachmentError> {
                self.$member.attach_ref_producer()
            }
        }
    };
}

/// A dynamically typed OQueue that allows attempting any kind of attachment, but dynamically
/// returns errors for unsupported ones.
pub struct AnyOQueueRef<T: ?Sized> {
    inner: Arc<implementation::OQueueImplementation<T>>,
}

// Manually forward do that we control the the exposed methods from OQueueImplementation
impl_oqueue_base_forward!(AnyOQueueRef, inner, [+ ?Sized]);
impl_consumable_oqueue_forward!(AnyOQueueRef, inner);
impl_oqueue_forward!(AnyOQueueRef, inner, [+ ?Sized]);

/// A reference to an OQueue of an unknown kind, meaning only observation is allowed.
#[derive(Clone)]
pub struct OQueueBaseRef<T> {
    inner: Arc<implementation::OQueueImplementation<T>>,
}

// Manually forward do that we control the the exposed methods from OQueueImplementation
impl_oqueue_base_forward!(OQueueBaseRef, inner, []);

/// A reference to an OQueue which supports consumers. These OQueues support publication is by value
/// so that ownership of the message can be transferred to the consumer.
#[derive(Clone)]
pub struct ConsumableOQueueRef<T: 'static> {
    inner: Arc<implementation::OQueueImplementation<T>>,
}

impl<T> ConsumableOQueueRef<T> {
    /// Create a new OQueue with the specified buffer length and support for produce by value and
    /// consumers.
    pub fn new(len: usize) -> Self {
        Self {
            inner: Arc::new(implementation::OQueueImplementation::new(len, true)),
        }
    }
}

// Manually forward do that we control the the exposed methods from OQueueImplementation
impl_oqueue_base_forward!(ConsumableOQueueRef, inner, []);
impl_consumable_oqueue_forward!(ConsumableOQueueRef, inner);

/// A reference to an observation OQueue. Observation OQueues do not have consumers and can publish
/// values by reference.
#[derive(Clone)]
pub struct OQueueRef<T: ?Sized + 'static> {
    inner: Arc<implementation::OQueueImplementation<T>>,
}

impl<T: ?Sized + 'static> OQueueRef<T> {
    /// Create a new observation OQueue with the specified buffer length.
    pub fn new(len: usize) -> Self {
        Self {
            inner: Arc::new(implementation::OQueueImplementation::new(len, false)),
        }
    }
}

// Manually forward do that we control the the exposed methods from OQueueImplementation
impl_oqueue_base_forward!(OQueueRef, inner, [+ ?Sized]);
impl_oqueue_forward!(OQueueRef, inner, [+ ?Sized]);

/// An attachment to an OQueue which allows transferring ownership of messages from the producer to
/// the consumer without copying or cloning.
pub struct ValueProducer<T> {
    oqueue: Arc<implementation::OQueueImplementation<T>>,
}

impl<T: Send + 'static> ValueProducer<T> {
    /// Produce a value, giving up ownership. This is used to pass an object to the consumer.
    pub fn produce(&self, v: T) {
        self.oqueue.produce(v)
    }

    /// Try to produce a value without blocking. Returns `Err(v)` if the operation would block,
    /// `None` if the value was successfully produced.
    pub fn try_produce(&self, v: T) -> Result<(), T> {
        self.oqueue.try_produce(v)
    }
}

/// An attachment to an OQueue which allows producing values values by reference for observation.
/// There can be no consumers since the message is not moved into the OQueue.
pub struct RefProducer<T: ?Sized> {
    oqueue: Arc<implementation::OQueueImplementation<T>>,
}

impl<T: Send + ?Sized + 'static> RefProducer<T> {
    /// Produce a value for observation. The value can be taken by reference, since only observers
    /// are allowed, meaning that only values extracted from the the value need to be stored.
    pub fn produce_ref(&self, v: &T) {
        self.oqueue.produce_ref(v)
    }

    /// Try to produce a value for observation without blocking. Returns `false` if the operation
    /// would block, `true` if the value was successfully produced.
    pub fn try_produce_ref(&self, v: &T) -> bool {
        self.oqueue.try_produce_ref(v)
    }
}

/// An attachment to an OQueue which allows consuming values from the OQueue. Consuming the value
/// takes ownership.
pub struct Consumer<T: 'static> {
    oqueue: Arc<implementation::OQueueImplementation<T>>,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.oqueue.detach_consumer();
    }
}

impl<T: Send + 'static> Blocker for Consumer<T> {
    fn should_try(&self) -> bool {
        self.oqueue.can_consume()
    }

    fn enqueue(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }

    fn remove(&self, key: WakerKey) {
        self.oqueue.read_wait_queue.remove(key);
    }
}

impl<T: Send + 'static> Consumer<T> {
    /// Consume a value from the queue, taking ownership of that value.
    pub fn consume(&self) -> T {
        self.oqueue.consume()
    }

    /// Try to consume a value without blocking. Returns `None` if no value is available.
    pub fn try_consume(&self) -> Option<T> {
        self.oqueue.try_consume()
    }

    /// Register a function to be called by the OQueue to observe each message.
    ///
    /// This consumes `self`, because the messages will now be consumed via a different mechanism.
    /// This will return an error if switching to inline is impossible or meaningless, for example
    /// if there are other consumers present.
    ///
    /// ## WARNING
    ///
    /// `f` **must** be fast and non-blocking. It will be run on the producers thread.
    pub fn consume_inline(
        self,
        f: impl Fn(T) + Send + 'static,
    ) -> Result<InlineConsumer<T>, AttachmentError> {
        self.oqueue.attach_inline_consumer(f)?;
        Ok(InlineConsumer {
            oqueue: self.oqueue.clone(),
            _phantom: PhantomData,
        })
    }
}

/// A handle to an inline consumer. That consumer will be detached and no longer called when this is
/// dropped.
pub struct InlineConsumer<T: 'static> {
    oqueue: Arc<implementation::OQueueImplementation<T>>,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl<T> Drop for InlineConsumer<T> {
    fn drop(&mut self) {
        self.oqueue.detach_inline_consumer();
    }
}

/// A function to convert a `StrongObserver` into an `InlineStrongObserver`. This erases the message
/// type `T` by having the function itself perform a downcast to the specific types. The function
/// will also use the observer key to get the query function used by the existing non-inline strong
/// observer.
///
/// This is always an instance of
/// [`implementation::OQueueImplementation::convert_strong_observer_to_inline`].
type ConvertToInlineFn<U> = fn(
    &(dyn implementation::UntypedOQueueImplementation + 'static),
    ObserverKey,
    Box<dyn Fn(&U) + Send + 'static>,
) -> Result<InlineObserverKey, AttachmentError>;

/// An attachment to an OQueue which allows observing events from the OQueue.
pub struct StrongObserver<U> {
    oqueue: Arc<dyn implementation::UntypedOQueueImplementation>,
    observer_id: ObserverKey,
    convert_to_inline: ConvertToInlineFn<U>,
    _phantom: PhantomData<core::cell::Cell<U>>,
}

impl<U> Drop for StrongObserver<U> {
    fn drop(&mut self) {
        self.oqueue.detach_strong_observer(self.observer_id);
    }
}

impl<U> Blocker for StrongObserver<U> {
    fn should_try(&self) -> bool {
        self.oqueue.can_strong_observe(self.observer_id)
    }

    fn enqueue(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey {
        self.oqueue.enqueue_read_waker(waker)
    }

    fn remove(&self, key: WakerKey) {
        self.oqueue.remove_read_waker(key)
    }
}

impl<U: Copy + Send + 'static> StrongObserver<U> {
    /// Observe a value from the queue. This value will have been extracted from the message by the
    /// query provided on attachment.
    pub fn strong_observe(&self) -> Result<U, ObservationError> {
        let mut ret = MaybeUninit::uninit();

        // SAFETY: U matches the call to `attach_strong_observer` which created self and
        // `observer_id`.
        unsafe {
            self.oqueue.strong_observe_into(
                self.observer_id,
                TypeId::of::<U>(),
                ret.as_mut_ptr() as *mut (),
            )
        }?;

        // SAFETY: `strong_observe_into` always fills `ret` if it returns Ok.
        unsafe { Ok(ret.assume_init()) }
    }

    /// Try to observe a value without blocking. Returns `None` if no value is available.
    pub fn try_strong_observe(&self) -> Result<Option<U>, ObservationError> {
        let mut ret = MaybeUninit::uninit();

        // SAFETY: U matches the call to `attach_strong_observer` which created self and
        // `observer_id`.
        let success = unsafe {
            self.oqueue.try_strong_observe_into(
                self.observer_id,
                TypeId::of::<U>(),
                ret.as_mut_ptr() as *mut (),
            )
        }?;
        if success {
            // SAFETY: try_strong_observe_into returned Ok(true).
            Ok(Some(unsafe { ret.assume_init() }))
        } else {
            Ok(None)
        }
    }

    /// Register a function to be called by the OQueue to observe each message.
    ///
    /// This consumes `self`, because the messages will now be observed via a different mechanism.
    /// This will return an error if switching to inline is impossible.
    ///
    ///  ## WARNING
    ///
    /// `f` **must** be fast and non-blocking. It will be run on the producers thread.
    pub fn strong_observe_inline(
        self,
        f: impl Fn(&U) + Send + 'static,
    ) -> Result<InlineStrongObserver, AttachmentError> {
        let inline_observer_id =
            (self.convert_to_inline)(self.oqueue.as_ref(), self.observer_id, Box::new(f))?;
        Ok(InlineStrongObserver {
            oqueue: self.oqueue.clone(),
            inline_observer_id,
            _phantom: PhantomData,
        })
    }
}

/// A handle for an inline strong observer. The strong observer function will no longer be called
/// when this value is dropped.
pub struct InlineStrongObserver {
    oqueue: Arc<dyn implementation::UntypedOQueueImplementation>,
    inline_observer_id: InlineObserverKey,
    _phantom: PhantomData<core::cell::Cell<()>>,
}

impl Drop for InlineStrongObserver {
    fn drop(&mut self) {
        self.oqueue
            .detach_inline_strong_observer(self.inline_observer_id);
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

/// An attachment to an OQueue which allows reading values from the history of the OQueue.
pub struct WeakObserver<U: Copy + Send> {
    oqueue: Arc<dyn implementation::UntypedOQueueImplementation>,
    observer_id: ObserverKey,
    last_observed: Cell<Cursor>,
    _phantom: PhantomData<Cell<U>>,
}

impl<U: Copy + Send + 'static> WeakObserver<U> {
    /// Wait until new values are available (that is values that have not yet been observed by some
    /// method on `self`). This does not guarantee that the caller will be able to observe those
    /// specific values. Only, that the caller will run eventually when new values are produced.
    pub fn wait(&self) {
        self.oqueue.wait(self.observer_id, self.last_observed.get());
    }

    /// Get the cursor for most recent value in the OQueue. This is concurrent and the returned
    /// value is immediately stale.
    pub fn newest_cursor(&self) -> Cursor {
        self.oqueue.newest_cursor(self.observer_id)
    }

    /// Get the cursor for oldest value still in the OQueue when this is called. This is concurrent
    /// and the returned value is immediately stale.
    pub fn oldest_cursor(&self) -> Cursor {
        self.oqueue.oldest_cursor(self.observer_id)
    }

    /// Get the value at a specific cursor if it is still available. It will always return the value
    /// requested or `None`.
    pub fn weak_observe(&self, i: Cursor) -> Result<Option<U>, ObservationError> {
        self.last_observed.set(self.last_observed.get().max(i));
        let mut ret = MaybeUninit::uninit();

        // SAFETY: U matches the call to `attach_weak_observer` which created self and
        // `observer_id`.
        let success = unsafe {
            self.oqueue.weak_observe_into(
                self.observer_id,
                TypeId::of::<U>(),
                i,
                ret.as_mut_ptr() as *mut (),
            )
        }?;
        if success {
            // SAFETY: try_strong_observe_into returned Ok(true).
            Ok(Some(unsafe { ret.assume_init() }))
        } else {
            Ok(None)
        }
    }

    /// Observe the most recent `n` values. Any values that cannot be accessed (due to the history
    /// being too short or them being concurrently overwritten) will be replaced by `None`. The
    /// returned `Vec` always has length `n`.
    pub fn weak_observe_recent(&self, n: usize) -> Result<Vec<Option<U>>, ObservationError> {
        let mut ret: Vec<_> = vec![None; n];
        self.weak_observe_recent_into(&mut ret)?;
        Ok(ret)
    }

    /// Observe the most recent `buf.len()` values. Any values that cannot be accessed (due to the
    /// history being too short or them being concurrently overwritten) will be replaced by `None`.
    pub fn weak_observe_recent_into(&self, buf: &mut [Option<U>]) -> Result<(), ObservationError> {
        let recent = self.newest_cursor();
        for i in 0..buf.len() {
            buf[i] = self.weak_observe(recent - (buf.len() - i - 1))?;
        }
        Ok(())
    }
}

#[cfg(ktest)]
mod test {
    use alloc::string::String;
    use core::assert_matches::assert_matches;

    use super::*;
    use crate::{orpc::oqueue::generic_test, prelude::*};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Message {
        id: u32,
        payload: String,
    }

    fn new_message(id: u32, payload: &str) -> Message {
        Message {
            id,
            payload: String::from(payload),
        }
    }

    #[ktest]
    fn consumable_oqueue_consume() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let consumer = queue.attach_consumer().unwrap();

        producer.produce(new_message(7, "hello"));
        let consumed = consumer.consume();

        assert_eq!(consumed, new_message(7, "hello"));
    }

    #[ktest]
    fn consumable_oqueue_strong_observe() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        producer.produce(new_message(3, "world"));

        let observed_id = observer.strong_observe().unwrap();

        assert_eq!(observed_id, 3);
    }

    #[ktest]
    fn consumable_oqueue_weak_observe_recent() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let observer = queue
            .attach_weak_observer(1, ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        producer.produce(new_message(1, "abcde"));

        let observed = observer.weak_observe_recent(1).unwrap();

        assert_eq!(observed, vec![Some(1)]);
    }

    #[ktest]
    fn oqueue_strong_observe() {
        let queue = OQueueRef::new(4);
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        producer.produce_ref(&new_message(3, "world"));

        let observed_id = observer.strong_observe().unwrap();

        assert_eq!(observed_id, 3);
    }

    #[ktest]
    fn oqueue_strong_observe_unsized() {
        let msg = &[1, 2, 3];
        let queue = OQueueRef::<[u32]>::new(4);
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &[u32]| m.len()))
            .unwrap();

        producer.produce_ref(msg);

        let observed_id = observer.strong_observe().unwrap();

        assert_eq!(observed_id, 3);
    }

    fn setup_for_weak_observation(history: usize) -> (RefProducer<Message>, WeakObserver<u32>) {
        let queue = OQueueRef::new(4);
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_weak_observer(history, ObservationQuery::new(|m: &Message| m.id))
            .unwrap();
        (producer, observer)
    }

    #[ktest]
    fn weak_observe_recent() {
        let (producer, observer) = setup_for_weak_observation(1);

        producer.produce_ref(&new_message(1, "abcde"));

        let observed = observer.weak_observe_recent(1).unwrap();

        assert_eq!(observed, vec![Some(1)]);
    }

    #[ktest]
    fn weak_observe_recent_into() {
        let (producer, observer) = setup_for_weak_observation(1);

        producer.produce_ref(&new_message(3, "len"));

        let mut buf = [None; 1];
        observer.weak_observe_recent_into(&mut buf).unwrap();

        assert_eq!(buf, [Some(3)]);
    }

    #[ktest]
    fn weak_observe_recent_multiple_values() {
        let (producer, observer) = setup_for_weak_observation(3);

        producer.produce_ref(&new_message(10, "a"));
        producer.produce_ref(&new_message(11, "bb"));
        producer.produce_ref(&new_message(12, "ccc"));

        let observed = observer.weak_observe_recent(3).unwrap();

        assert_eq!(observed, vec![Some(10), Some(11), Some(12)]);

        let observed = observer.weak_observe_recent(1).unwrap();

        assert_eq!(observed, vec![Some(12)]);
    }

    #[ktest]
    fn weak_observe() {
        let (producer, observer) = setup_for_weak_observation(2);

        producer.produce_ref(&new_message(2, "xyz"));
        producer.produce_ref(&new_message(3, "xyz"));

        let old = observer.oldest_cursor();
        let recent = observer.newest_cursor();
        assert!(old <= recent);
        assert_eq!(recent - 1, old);

        let observed = observer.weak_observe(recent).unwrap();
        assert_eq!(observed, Some(3));
    }

    #[ktest]
    fn unsupported_consumer() {
        let queue = OQueueRef::<u32>::new(4);
        assert_matches!(
            queue.as_any_oqueue().attach_consumer().err().unwrap(),
            AttachmentError::Unsupported { .. }
        );
    }

    #[ktest]
    fn consumable_oqueue_strong_observe_filtered() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new_filter(|m: &Message| {
                if m.id % 2 == 0 { Some(m.id) } else { None }
            }))
            .unwrap();

        producer.produce(new_message(3, "skip"));
        producer.produce(new_message(4, "observe"));

        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, 4);
    }

    #[ktest]
    fn oqueue_weak_observe_filtered() {
        let (producer, observer) = setup_for_weak_observation_filtered();

        producer.produce_ref(&new_message(1, "a"));
        producer.produce_ref(&new_message(2, "bb"));
        producer.produce_ref(&new_message(3, "ccc"));

        let observed = observer.weak_observe_recent(1).unwrap();
        // Only id=2 passes filter (even id)
        assert_eq!(observed, vec![Some(2)]);
    }

    fn setup_for_weak_observation_filtered() -> (RefProducer<Message>, WeakObserver<u32>) {
        let queue = OQueueRef::new(4);
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_weak_observer(
                2,
                ObservationQuery::new_filter(
                    |m: &Message| {
                        if m.id % 2 == 0 { Some(m.id) } else { None }
                    },
                ),
            )
            .unwrap();
        (producer, observer)
    }

    #[ktest]
    fn consumable_oqueue_try_produce() {
        let queue = ConsumableOQueueRef::new(2);
        let producer = queue.attach_value_producer().unwrap();
        let _consumer = queue.attach_consumer().unwrap();

        let msg1 = new_message(1, "first");
        assert!(producer.try_produce(msg1.clone()).is_ok());

        let msg2 = new_message(2, "blocked");
        assert_eq!(
            producer.try_produce(msg2).unwrap_err(),
            new_message(2, "blocked")
        );
    }

    #[ktest]
    fn consumable_oqueue_try_consume() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let consumer = queue.attach_consumer().unwrap();

        assert_eq!(consumer.try_consume(), None);

        producer.produce(new_message(7, "hello"));
        assert_eq!(consumer.try_consume(), Some(new_message(7, "hello")));
        assert_eq!(consumer.try_consume(), None);
    }

    #[ktest]
    fn consumable_oqueue_try_strong_observe() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        assert_eq!(observer.try_strong_observe().unwrap(), None);

        producer.produce(new_message(5, "data"));
        assert_eq!(observer.try_strong_observe().unwrap(), Some(5));
        assert_eq!(observer.try_strong_observe().unwrap(), None);
    }

    #[ktest]
    fn consumable_oqueue_inline_consumer() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let consumer = queue.attach_consumer().unwrap();

        let received: Arc<SpinLock<alloc::vec::Vec<Message>>> = Arc::new(SpinLock::new(Vec::new()));

        let inline_consumer = consumer
            .consume_inline({
                let received = received.clone();
                move |msg| {
                    let mut vec = received.lock();
                    vec.push(msg);
                }
            })
            .unwrap();

        producer.produce(new_message(1, "first"));
        producer.produce(new_message(2, "second"));

        let result = received.lock();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], new_message(1, "first"));
        assert_eq!(result[1], new_message(2, "second"));

        drop(inline_consumer);
    }

    #[ktest]
    fn consumable_oqueue_inline_strong_observer() {
        let queue = ConsumableOQueueRef::new(4);
        let producer = queue.attach_value_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        let observed_ids: Arc<SpinLock<alloc::vec::Vec<u32>>> = Arc::new(SpinLock::new(Vec::new()));

        let inline_observer = observer
            .strong_observe_inline({
                let observed_ids = observed_ids.clone();
                move |id| {
                    let mut vec = observed_ids.lock();
                    vec.push(*id);
                }
            })
            .unwrap();

        producer.produce(new_message(10, "msg1"));
        producer.produce(new_message(20, "msg2"));
        producer.produce(new_message(30, "msg3"));

        let result = observed_ids.lock();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], 10);
        assert_eq!(result[1], 20);
        assert_eq!(result[2], 30);

        drop(inline_observer);
    }

    #[ktest]
    fn oqueue_inline_strong_observer() {
        let queue = OQueueRef::new(4);
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &Message| m.id))
            .unwrap();

        let observed_ids: Arc<SpinLock<alloc::vec::Vec<u32>>> = Arc::new(SpinLock::new(Vec::new()));

        let inline_observer = observer
            .strong_observe_inline({
                let observed_ids = observed_ids.clone();
                move |id| {
                    let mut vec = observed_ids.lock();
                    vec.push(*id);
                }
            })
            .unwrap();

        producer.produce_ref(&new_message(5, "ref1"));
        producer.produce_ref(&new_message(15, "ref2"));

        let result = observed_ids.lock();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], 5);
        assert_eq!(result[1], 15);

        drop(inline_observer);
    }

    #[ktest]
    fn consumable_oqueue_attach_inline_strong_observer() {
        let queue = ConsumableOQueueRef::<Message>::new(4);
        let producer = queue.attach_value_producer().unwrap();

        let observed_messages: Arc<SpinLock<alloc::vec::Vec<Message>>> =
            Arc::new(SpinLock::new(Vec::new()));

        let inline_observer = queue
            .attach_inline_strong_observer({
                let observed_messages = observed_messages.clone();
                move |msg| {
                    let mut vec = observed_messages.lock();
                    vec.push(msg.clone());
                }
            })
            .unwrap();

        producer.produce(new_message(7, "first"));
        producer.produce(new_message(8, "second"));
        producer.produce(new_message(9, "third"));

        let result = observed_messages.lock();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], new_message(7, "first"));
        assert_eq!(result[1], new_message(8, "second"));
        assert_eq!(result[2], new_message(9, "third"));

        drop(inline_observer);
    }

    #[ktest]
    fn oqueue_attach_inline_strong_observer() {
        let queue = OQueueRef::<Message>::new(4);
        let producer = queue.attach_ref_producer().unwrap();

        let observed_messages: Arc<SpinLock<alloc::vec::Vec<Message>>> =
            Arc::new(SpinLock::new(Vec::new()));

        let inline_observer = queue
            .attach_inline_strong_observer({
                let observed_messages = observed_messages.clone();
                move |msg| {
                    let mut vec = observed_messages.lock();
                    vec.push(msg.clone());
                }
            })
            .unwrap();

        producer.produce_ref(&new_message(100, "ref1"));
        producer.produce_ref(&new_message(101, "ref2"));

        let result = observed_messages.lock();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], new_message(100, "ref1"));
        assert_eq!(result[1], new_message(101, "ref2"));

        drop(inline_observer);
    }

    // TODO: Deduplicate the tests and remove the generic package. Probably move all tests into a
    // module file.

    #[ktest]
    fn generic_produce_consume() {
        generic_test::test_produce_consume(ConsumableOQueueRef::<generic_test::TestMessage>::new(
            2,
        ));
    }

    #[ktest]
    fn generic_produce_strong_observe() {
        generic_test::test_produce_strong_observe(
            ConsumableOQueueRef::<generic_test::TestMessage>::new(2),
        );
    }

    #[ktest]
    fn generic_produce_weak_observe() {
        generic_test::test_produce_weak_observe(
            ConsumableOQueueRef::<generic_test::TestMessage>::new(3),
        );
    }

    #[ktest]
    fn generic_send_receive_blocker() {
        let queue = ConsumableOQueueRef::<generic_test::TestMessage>::new(16);
        generic_test::test_send_receive_blocker(queue, 32, 3);
    }

    #[ktest]
    fn generic_produce_strong_observe_only() {
        generic_test::test_produce_strong_observe_only(ConsumableOQueueRef::<
            generic_test::TestMessage,
        >::new(2));
    }

    #[ktest]
    fn generic_consumer_late_attach() {
        generic_test::test_consumer_late_attach(
            ConsumableOQueueRef::<generic_test::TestMessage>::new(4),
        );
    }

    #[ktest]
    fn generic_consumer_detach() {
        generic_test::test_consumer_detach(ConsumableOQueueRef::<generic_test::TestMessage>::new(
            4,
        ));
    }

    #[ktest]
    fn generic_strong_observer_detach() {
        generic_test::test_strong_observer_detach(
            ConsumableOQueueRef::<generic_test::TestMessage>::new(4),
        );
    }

    #[ktest]
    fn generic_strong_observer_late_attach() {
        generic_test::test_strong_observer_late_attach(ConsumableOQueueRef::<
            generic_test::TestMessage,
        >::new(4));
    }
}
