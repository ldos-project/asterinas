// SPDX-License-Identifier: MPL-2.0

//! The internal implementation of OQueues.
//!
//! See [`OQueueImplementation`].
//!
//! ## Terminology
//!
//! * By convention, methods which place their output into an untyped buffer are named `*_into` and
//!   take a pointer parameter `dest: *mut ()`.

// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/155): Add support for sharing ring
// buffers between multiple observers.

use alloc::{alloc::AllocError, boxed::Box, sync::Arc};
use core::{
    any::{Any, TypeId},
    marker::PhantomData,
};

use log::warn;
use slotmap::{SlotMap, new_key_type};
use snafu::ensure;
use static_assertions::assert_obj_safe;

use crate::{
    orpc::{
        framework::{CurrentServer, errors::RPCError},
        oqueue::{
            AttachmentError, Cursor, InlineStrongObserver, ObservationError, ObservationQuery,
            ResourceUnavailableSnafu, single_thread_ring_buffer::RingBuffer,
        },
    },
    sync::{SpinLock, WaitQueue, WakerKey},
};

new_key_type! {
    /// A unique identifier for an observer ring buffer.
    pub struct ObserverKey;
    /// A unique identifier for an inline strong observer.
    pub struct InlineObserverKey;
}

/// The underlying implementation of OQueues. This is used within an `Arc` to reference the OQueue
/// internally. Externally, code will use [`super::OQueueRef`] and similar.
///
/// ## Adding implementations
///
/// This does not need to be a single unified implementation. This could be replaced by an enum of
/// different implementations and then have operations dispatched appropriately. This may be needed
/// to support locking and lock-free implementation. It could even provide a way to use a
/// hypothetical `dyn OQueueDynImplementation<T>` backend; however, `OQueueDynImplementation` would
/// require quite a few type-erased unsafe operations.
pub(crate) struct OQueueImplementation<T: ?Sized> {
    inner: SpinLock<OQueueInner<T>>,
    /// The size to use for the consumer and strong-observer ring-buffers.
    len: usize,
    supports_consume: bool,
    pub(super) put_wait_queue: WaitQueue,
    pub(super) read_wait_queue: WaitQueue,
}

impl<T: ?Sized + 'static> OQueueImplementation<T> {
    /// Create a new OQueue.
    ///
    /// * `len` is the ring buffer length used for consumers and strong-observers.
    /// * `supports_consume` specifies the attachment it allows later.
    pub(crate) fn new(mut len: usize, supports_consume: bool) -> Self {
        if len < 2 {
            warn!(
                "Creating an OQueue with length {len} is automatically increased to 2. Ring buffers smaller than 2 are not supported."
            );
            len = 2;
        }
        Self {
            inner: SpinLock::new(OQueueInner {
                consumer_ring_buffer: Default::default(),
                n_consumers: 0,
                observer_ring_buffers: Default::default(),
                inline_strong_observers: Default::default(),
                inline_consumer: None,
            }),
            len,
            supports_consume,
            put_wait_queue: WaitQueue::new(),
            read_wait_queue: WaitQueue::new(),
        }
    }

    /// Detach a consumer. This will free the consumer ring buffer if there are no consumers left.
    pub(super) fn detach_consumer(self: &Arc<Self>) {
        let mut inner = self.inner.lock();
        inner.n_consumers -= 1;
        if inner.n_consumers == 0 {
            inner.consumer_ring_buffer = None;
        }
    }

    pub(super) fn detach_inline_consumer(&self) {
        let mut inner = self.inner.lock();
        inner.inline_consumer = None;
    }

    /// Create a new ring buffer for values of type `U` and attach it to this `self` as an
    /// observation ring buffer.
    fn new_observation_ring_buffer<U>(
        self: &Arc<Self>,
        query: ObservationQuery<T, U>,
        len: usize,
        is_strong: bool,
    ) -> Result<ObserverKey, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let mut ring_buffer = ObservationRingBuffer::new::<U>(query, len)
            .map_err(|_| super::ResourceUnavailableSnafu.build())?;

        if is_strong {
            let id = ring_buffer.ring_buffer.new_strong_reader();
            // We don't reuse ring buffers, so for now this is always 0.
            assert_eq!(id, 0);
        }

        let mut inner = self.inner.lock();
        let observer_key = inner.observer_ring_buffers.insert(ring_buffer);
        Ok(observer_key)
    }

    /// Attach a strong observer.
    pub(super) fn attach_strong_observer<U>(
        self: &Arc<Self>,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::StrongObserver<U>, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let observer_key = self.new_observation_ring_buffer(query, self.len, true)?;
        let oqueue: Arc<dyn UntypedOQueueImplementation> = self.clone();
        Ok(super::StrongObserver {
            oqueue,
            observer_id: observer_key,
            convert_to_inline: Self::convert_strong_observer_to_inline,
            _phantom: PhantomData,
        })
    }

    /// Attach a strong observer to the OQueue by calling a strong observer function with each
    /// value.
    ///
    /// This cannot be combined with the below query based one because the "identity query" cannot
    /// exist for `T: !Copy`.
    pub(super) fn attach_inline_strong_observer(
        self: &Arc<Self>,
        f: impl Fn(&T) + Send + 'static,
    ) -> Result<InlineStrongObserver, super::AttachmentError> {
        let mut inner = self.inner.lock();
        let key = inner.inline_strong_observers.insert(wrap_closure_ref(f));
        Ok(super::InlineStrongObserver {
            oqueue: self.clone(),
            inline_observer_id: key,
            _phantom: PhantomData,
        })
    }

    /// Attach a strong observer to the OQueue by calling a strong observer function with each
    /// value. The value is passed through a query. This is used for converting a strong-observer
    /// attachment into an inline.
    ///
    /// This removes the ring buffer associated with `observer_id` so it is no longer valid (and may
    /// be reallocated). This should *only* be called from something which take ownership of
    /// observer_id and converts it into the new form.
    ///
    /// This returns a new ID associated with the registered inline attachment. This will not be the
    /// same as the argument.
    pub(super) fn convert_strong_observer_to_inline<U: Send + Copy + 'static>(
        this: &dyn UntypedOQueueImplementation,
        observer_id: ObserverKey,
        f: Box<dyn Fn(&U) + Send + 'static>,
    ) -> Result<InlineObserverKey, super::AttachmentError> {
        let this: &Self = (this as &dyn Any).downcast_ref().unwrap();
        let mut inner = this.inner.lock();

        // Move the ring buffer out
        let mut observation_ring_buffer = inner
            .observer_ring_buffers
            .remove(observer_id)
            .expect("tried to attach inline strong observer for observer_id which did not exist");
        let query = (observation_ring_buffer.query as Box<dyn Any>)
            .downcast::<ObservationQuery<T, U>>()
            .expect("tried to attach inline strong observer with a different type from original attachment");

        // Drain the ring buffer into the function.
        // TODO: Use a real head_id when we share ring buffers.
        let head_id = 0;
        while let Some(v) = observation_ring_buffer
            .ring_buffer
            .try_strong_observe::<U>(head_id)
        {
            f(&v)
        }

        // Register the actual handler
        let key = inner
            .inline_strong_observers
            .insert(wrap_closure_ref(move |v| {
                if let Some(v) = query.call(v) {
                    f(&v)
                }
            }));

        Ok(key)
    }

    /// Attach a weak observer.
    pub(super) fn attach_weak_observer<U>(
        self: &Arc<Self>,
        history_len: usize,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::WeakObserver<U>, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let observer_key = self.new_observation_ring_buffer(query, history_len, false)?;
        Ok(super::WeakObserver {
            oqueue: self.clone(),
            observer_id: observer_key,
            last_observed: Cursor(0).into(),
            _phantom: PhantomData,
        })
    }

    /// Create a new reference to `self` without its OQueue kind.
    pub(super) fn as_any_oqueue(self: &Arc<Self>) -> super::AnyOQueueRef<T> {
        super::AnyOQueueRef {
            inner: self.clone(),
        }
    }
}

/// Wrap a closure to run in the context of the current server if there is one.
fn wrap_closure_ref<T: ?Sized + 'static>(
    f: impl Fn(&T) + Send + 'static,
) -> Box<dyn Fn(&T) + Send> {
    // TODO(arthurp): This embeds a detail of ORPC in the middle of the OQueue implementation. It
    // also forces this overhead on every closure regardless of it's origin.
    if let Some(s) = CurrentServer::current_cloned() {
        let f: Box<dyn Fn(&T) + Send + 'static> = Box::new(move |v| {
            let _ = s.orpc_server_base().call_in_context::<_, RPCError>(|| {
                f(v);
                Ok(())
            });
        });
        f
    } else {
        Box::new(f)
    }
}

impl<T: ?Sized + 'static> OQueueImplementation<T> {
    /// True if try_produce into self is *expected* to succeed. It may still fail if there is a
    /// concurrent producer.
    fn can_produce(&self) -> bool {
        self.inner.lock().can_produce()
    }

    /// Produce a value by reference, blocking until it completes.
    pub(super) fn produce_ref(&self, v: &T) {
        self.put_wait_queue.wait_until(|| {
            if self.try_produce_ref(v) {
                Some(())
            } else {
                None
            }
        })
    }

    /// Attempt to produce a value by reference. This executes all the queries on `v` and places the
    /// query results into the appropriate observation ring buffers.
    pub(super) fn try_produce_ref(&self, v: &T) -> bool {
        let mut inner = self.inner.lock();
        assert!(inner.consumer_ring_buffer.is_none());
        if inner.can_produce() {
            for ObservationRingBuffer {
                query, ring_buffer, ..
            } in inner.observer_ring_buffers.values_mut()
            {
                query.call_into(v, ring_buffer);
            }
            for f in inner.inline_strong_observers.values() {
                f(v)
            }
            drop(inner);

            self.read_wait_queue.wake_all();
            true
        } else {
            false
        }
    }

    /// Attach an producer expecting references to the OQueue if it has no consumers.
    pub(super) fn attach_ref_producer(
        self: &Arc<Self>,
    ) -> Result<super::RefProducer<T>, super::AttachmentError> {
        if self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        Ok(super::RefProducer {
            oqueue: self.clone(),
        })
    }
}

impl<T: Send + 'static> OQueueImplementation<T> {
    /// Produce into the OQueue. Blocking until space is available
    pub(super) fn produce(&self, v: T) {
        let mut v = v;
        loop {
            if let Err(ret) = self.try_produce(v) {
                v = ret;
                // TODO: PERFORMANCE: Using can_produce here and try_produce above requires 2
                // acquires.
                self.put_wait_queue
                    .wait_until(|| if self.can_produce() { Some(()) } else { None })
            } else {
                return;
            }
        }
    }

    /// Attempt to produce into the OQueue. This either succeeds or returns the value that would
    /// have been produced.
    pub(super) fn try_produce(&self, v: T) -> Result<(), T> {
        let mut inner = self.inner.lock();
        if inner.can_produce() {
            for ObservationRingBuffer {
                query, ring_buffer, ..
            } in inner.observer_ring_buffers.values_mut()
            {
                query.call_into(&v, ring_buffer);
            }
            for f in inner.inline_strong_observers.values() {
                f(&v)
            }
            assert!(inner.consumer_ring_buffer.is_none() || inner.inline_consumer.is_none());
            if let Some(ring_buffer) = &mut inner.consumer_ring_buffer {
                let v = ring_buffer.try_produce(v);
                assert!(v.is_none());
            } else if let Some(consumer) = &inner.inline_consumer {
                consumer(v);
            }
            drop(inner);

            self.read_wait_queue.wake_all();
            Ok(())
        } else {
            Err(v)
        }
    }

    /// Consume a value, blocking until it completes.
    pub(super) fn consume(&self) -> T {
        self.read_wait_queue.wait_until(|| self.try_consume())
    }

    pub(super) fn can_consume(&self) -> bool {
        let inner = self.inner.lock();
        inner
            .consumer_ring_buffer
            .as_ref()
            .expect("consume not supported")
            .can_get_for_head(0)
    }

    /// Attempt to consume a value from the consumer ring buffer, taking ownership of the value.
    pub(super) fn try_consume(&self) -> Option<T> {
        let mut inner = self.inner.lock();
        // SAFETY: The consumer ring buffer is never used for observation.
        let ret = unsafe {
            inner
                .consumer_ring_buffer
                .as_mut()
                .expect("consume not supported")
                .try_consume()
        };
        drop(inner);
        if ret.is_some() {
            self.put_wait_queue.wake_all();
        }
        ret
    }

    /// Attach a by-value producer to the OQueue if this supports consumers.
    pub(super) fn attach_value_producer(
        self: &Arc<Self>,
    ) -> Result<super::ValueProducer<T>, super::AttachmentError> {
        if !self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        Ok(super::ValueProducer {
            oqueue: self.clone(),
        })
    }

    /// Attach a consumer to the OQueue if this does not support consumers.
    pub(super) fn attach_consumer(
        self: &Arc<Self>,
    ) -> Result<super::Consumer<T>, super::AttachmentError> {
        if !self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        let mut inner = self.inner.lock();
        if inner.consumer_ring_buffer.is_none() {
            let mut ring_buffer = RingBuffer::new::<T>(self.len)
                .map_err(|_| super::ResourceUnavailableSnafu.build())?;
            let id = ring_buffer.new_strong_reader();
            assert_eq!(id, 0);
            inner.consumer_ring_buffer = Some(ring_buffer);
        }
        inner.n_consumers += 1;
        drop(inner);
        Ok(super::Consumer {
            oqueue: self.clone(),
            _phantom: PhantomData,
        })
    }

    pub(super) fn attach_inline_consumer(
        self: &Arc<Self>,
        f: impl Fn(T) + Send + 'static,
    ) -> Result<(), AttachmentError> {
        let mut inner = self.inner.lock();
        ensure!(inner.inline_consumer.is_none(), ResourceUnavailableSnafu);
        inner.inline_consumer = Some(Box::new(f));
        Ok(())
    }
}

/// The part of [`OQueueImplementation`] which is protected by a lock.
struct OQueueInner<T: ?Sized> {
    /// The ring buffer used for consumers.
    consumer_ring_buffer: Option<RingBuffer>,
    /// The number of attached consumers. This is required to detect when the `consumer_ring_buffer`
    /// can be discarded.
    n_consumers: usize,
    /// The ring buffers used for observers. If a ring buffer is no longer needed, due to a
    /// detachment, its slot will be set to `None` and ignored. `None` slots are reused for later
    /// attachments.
    observer_ring_buffers: SlotMap<ObserverKey, ObservationRingBuffer<T>>,
    // TODO: PERFORMANCE: There will be a bunch of cases where there is *only* a consumer ring
    // buffer, or *only* a single observer ring. It would be nice if either could be inlined. As of
    // now, the consumer is always inlined even when it's not in use, and the observer never is.
    // TODO: PERFORMANCE: This needs a way to share ring buffers when multiple consumers use the
    // same query.
    /// Inline strong observers of the message type. These will be called during production.
    #[expect(clippy::type_complexity)]
    inline_strong_observers: SlotMap<InlineObserverKey, Box<dyn Fn(&T) + Send>>,
    /// The inline consume if there is one.
    inline_consumer: Option<Box<dyn Fn(T) + Send>>,
}

impl<T: ?Sized> OQueueInner<T> {
    /// True if all ring buffers can produce.
    fn can_produce(&self) -> bool {
        self.observer_ring_buffers
            .values()
            .all(|r| r.ring_buffer.can_produce())
            && self.consumer_ring_buffer.iter().all(|r| r.can_produce())
    }
}

/// A partially type erased [`ObservationQuery`] with `U` (the query output type) erased, but `T`
/// (the message type) retained statically. This is used in the OQueue machinery to dispatch to the
/// query and produce it's result into a [`RingBuffer`].
trait ErasedObservationQuery<T: ?Sized>: Send + Any {
    /// Call the query and then produce the value into the provided ring buffer. The ring buffer
    /// must have a type matching the result of the query (`U` below). Returns false if the value
    /// could not be produced into `ring_buffer`.
    fn call_into(&self, v: &T, ring_buffer: &mut RingBuffer) -> bool;
}

impl<T: ?Sized + 'static, U: Send + 'static> ErasedObservationQuery<T> for ObservationQuery<T, U> {
    fn call_into(&self, v: &T, ring_buffer: &mut RingBuffer) -> bool {
        if let Some(v) = self.call(v) {
            ring_buffer.try_produce(v).is_none()
        } else {
            true
        }
    }
}

/// A wrapper over [`RingBuffer`] which provides the information needed to perform produce and
/// observe operations without exposing the type of the observed value (`U`).
struct ObservationRingBuffer<T: ?Sized> {
    // TODO: PERFORMANCE: Replace the dyn ref and optimize based on the known structure of
    // ObservationQuery. This should remove a level of pointer indirection.
    /// The query to filter `T` and place the result into the ring buffer.
    query: Box<dyn ErasedObservationQuery<T>>,
    /// Function to extract a value from the ring buffer with strong observer semantics and place it
    /// in the memory pointed to by `dest`. This indirection is needed since the type `U` in the
    /// ring buffer is not statically known.
    ///
    /// ## Safety
    ///
    /// `dest` must point to memory appropriate for the type `U` with which `ring_buffer` was
    /// constructed.
    try_strong_observe_into: unsafe fn(ring_buffer: &mut RingBuffer, dest: *mut ()) -> bool,
    /// Function to extract a value from the ring buffer and place it in the memory pointed to by
    /// `dest`. This indirection is needed since the type `U` in the ring buffer is not statically
    /// known.
    ///
    /// ## Safety
    ///
    /// `dest` must point to memory appropriate for the type `U` with which `ring_buffer` was
    /// constructed.
    weak_observe_into: unsafe fn(ring_buffer: &mut RingBuffer, Cursor, dest: *mut ()) -> bool,
    /// The actual ring buffer storing the data.
    ring_buffer: RingBuffer,
}

impl<T: ?Sized + 'static> ObservationRingBuffer<T> {
    /// Create a new ring buffer for a message type `T` and an observed type of `U`. This will be
    /// used to store values of type `U` for observation.
    fn new<U: Copy + Send + 'static>(
        query: ObservationQuery<T, U>,
        len: usize,
    ) -> Result<Self, AllocError> {
        let ring_buffer = RingBuffer::new::<U>(len)?;

        unsafe fn try_strong_observe_into<U: Copy + Send + 'static>(
            r: &mut RingBuffer,
            d: *mut (),
        ) -> bool {
            // TODO: Use a real head_id when we share ring buffers.
            let head_id = 0;
            if let Some(v) = r.try_strong_observe::<U>(head_id) {
                // SAFETY: The safety requirements of ObservationRingBuffer::try_strong_observe_into
                // specify that d must be appropriate for a value of type U.
                unsafe {
                    (d as *mut U).write(v);
                }
                true
            } else {
                false
            }
        }

        unsafe fn weak_observe_into<U: Copy + Send + 'static>(
            r: &mut RingBuffer,
            c: Cursor,
            d: *mut (),
        ) -> bool {
            if let Some(v) = r.try_weak_observe::<U>(c) {
                // SAFETY: The safety requirements of ObservationRingBuffer::weak_observe_into
                // specify that d must be appropriate for a value of type U.
                unsafe {
                    (d as *mut U).write(v);
                }
                true
            } else {
                false
            }
        }

        Ok(ObservationRingBuffer {
            query: Box::new(query),
            try_strong_observe_into: try_strong_observe_into::<U>,
            weak_observe_into: weak_observe_into::<U>,
            ring_buffer,
        })
    }
}

/// A dyn-compatible trait to type erase `T` from [`OQueueImplementation<T>`]. The operations here
/// conceptually take the observed type `U` as a type parameter, but this is not possible in a
/// dyn-compatible trait.
///
/// TODO(arthurp): PERFORMANCE: These methods could be lifted out of the [`OQueueImplementation`]
/// and written directly on the untyped representation, since they are not actually going to involve
/// `T`. This would avoid an indirect function call and allow inlining. This would require using
/// `repr(C)` and direct manipulation of the representation.
pub(super) trait UntypedOQueueImplementation: Sync + Send + Any {
    /// Release any resources held by observer with the given ID. Using that `observer_id` again may
    /// produce UB.
    fn detach_strong_observer(&self, observer_id: ObserverKey);

    /// Release any resources held by inline observer with the given key.
    fn detach_inline_strong_observer(&self, inline_observer_id: InlineObserverKey);

    fn can_strong_observe(&self, observer_id: ObserverKey) -> bool;

    fn enqueue_read_waker(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey;

    fn remove_read_waker(&self, key: WakerKey);

    /// Copy the next value available to the specified observer into `dest` if it is available. This
    /// returns `Ok(true)` if the value was copied, `Ok(false)` if there was not value available
    /// yet, and an error if some other failure happened.
    ///
    /// ## Safety
    ///
    /// The caller must guarantee that:
    ///
    /// * `dest` points to memory which is of the correct size and alignment for a value of the type
    ///   with the provided `type_id`.
    /// * `type_id` matches the type of buffer created in `attach_strong_observer` when it returned
    ///   this `observer_id`.
    ///
    /// After this returns, `dest` will be initialed iff this returns `Ok(true)`.
    ///
    /// NOTE: `type_id` may not actually be used in release mode (which is why it must match the
    /// observer_id). However, in debug mode it may be checked to catch bugs in the handles.
    unsafe fn try_strong_observe_into(
        &self,
        observer_id: ObserverKey,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<bool, ObservationError>;

    /// A blocking version of [`UntypedOQueueInner::try_strong_observe_into`]. All the safety
    /// requirements on that apply here, except that this will never return `Ok` without filling
    /// `dest`, since it blocks waiting for a value.
    unsafe fn strong_observe_into(
        &self,
        observer_id: ObserverKey,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), ObservationError>;

    /// Copy the value at index `cursor` into `dest` if it is available. This returns `Ok(true)` if
    /// the value was copied, `Ok(false)` if there was not value available, and an error if some
    /// other failure happened.
    ///
    /// ## Safety
    ///
    /// The caller must guarantee that:
    ///
    /// * `dest` points to memory which is of the correct size and alignment for a value of the type
    ///   with the provided `type_id`.
    /// * `type_id` matches the type of buffer created in `attach_weak_observer` when it returned
    ///   this `observer_id`.
    ///
    /// After this returns, `dest` will be initialed iff this returns `Ok(true)`.
    ///
    /// NOTE: `type_id` may not actually be used in release mode (which is why it must match the
    /// observer_id). However, in debug mode it may be checked to catch bugs in the handles.
    unsafe fn weak_observe_into(
        &self,
        observer_id: ObserverKey,
        type_id: TypeId,
        cursor: Cursor,
        dest: *mut (),
    ) -> Result<bool, ObservationError>;

    /// Wait until new values are available for the specific observer.
    fn wait(&self, observer_id: ObserverKey, cursor: Cursor);

    /// Get the most recent cursor which will return a value.
    fn newest_cursor(&self, observer_id: ObserverKey) -> Cursor;

    /// Get the oldest cursor which will return a value.
    fn oldest_cursor(&self, observer_id: ObserverKey) -> Cursor;
}

assert_obj_safe!(UntypedOQueueImplementation);

impl<T: ?Sized + 'static> UntypedOQueueImplementation for OQueueImplementation<T> {
    fn detach_strong_observer(&self, observer_id: ObserverKey) {
        let mut inner = self.inner.lock();
        inner.observer_ring_buffers.remove(observer_id);
    }

    fn detach_inline_strong_observer(&self, inline_observer_id: InlineObserverKey) {
        let mut inner = self.inner.lock();
        inner.inline_strong_observers.remove(inline_observer_id);
    }

    unsafe fn try_strong_observe_into(
        &self,
        observer_id: ObserverKey,
        _type_id: TypeId,
        dest: *mut (),
    ) -> Result<bool, ObservationError> {
        let mut inner = self.inner.lock();
        let ObservationRingBuffer {
            try_strong_observe_into,
            ring_buffer,
            ..
        } = inner
            .observer_ring_buffers
            .get_mut(observer_id)
            .expect("should only be called with an id returned from new_observation_ring_buffer");

        // SAFETY: weak_observe_into and ring_buffer where created together with the same type U.
        let ret = unsafe { try_strong_observe_into(ring_buffer, dest) };
        drop(inner);
        if ret {
            self.put_wait_queue.wake_all();
        }
        Ok(ret)
    }

    unsafe fn strong_observe_into(
        &self,
        observer_id: ObserverKey,
        _type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), ObservationError> {
        self.read_wait_queue.wait_until(|| {
            // SAFETY: The requirements of try_strong_observe_into are the same as this function.
            let r = unsafe { self.try_strong_observe_into(observer_id, _type_id, dest) };
            if let Ok(false) = r { None } else { Some(r) }
        })?;
        Ok(())
    }

    unsafe fn weak_observe_into(
        &self,
        observer_id: ObserverKey,
        _type_id: TypeId,
        cursor: Cursor,
        dest: *mut (),
    ) -> Result<bool, ObservationError> {
        let mut inner = self.inner.lock();
        let ObservationRingBuffer {
            weak_observe_into,
            ring_buffer,
            ..
        } = inner
            .observer_ring_buffers
            .get_mut(observer_id)
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        // SAFETY: weak_observe_into and ring_buffer where created together with the same type U.
        Ok(unsafe { weak_observe_into(ring_buffer, cursor, dest) })
    }

    fn wait(&self, observer_id: ObserverKey, cursor: Cursor) {
        self.read_wait_queue.wait_until(|| {
            if self.newest_cursor(observer_id) > cursor {
                Some(())
            } else {
                None
            }
        })
    }

    fn newest_cursor(&self, observer_id: ObserverKey) -> Cursor {
        let mut inner = self.inner.lock();
        let ObservationRingBuffer { ring_buffer, .. } = inner
            .observer_ring_buffers
            .get_mut(observer_id)
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        ring_buffer.newest_cursor()
    }

    fn oldest_cursor(&self, observer_id: ObserverKey) -> Cursor {
        let mut inner = self.inner.lock();
        let ObservationRingBuffer { ring_buffer, .. } = inner
            .observer_ring_buffers
            .get_mut(observer_id)
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        ring_buffer.oldest_cursor()
    }

    fn can_strong_observe(&self, observer_id: ObserverKey) -> bool {
        let mut inner = self.inner.lock();
        let ObservationRingBuffer { ring_buffer, .. } = inner
            .observer_ring_buffers
            .get_mut(observer_id)
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        let head_id = 0;
        ring_buffer.can_get_for_head(head_id)
    }

    fn enqueue_read_waker(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey {
        self.read_wait_queue.enqueue(waker.clone())
    }

    fn remove_read_waker(&self, key: WakerKey) {
        self.read_wait_queue.remove(key);
    }
}
