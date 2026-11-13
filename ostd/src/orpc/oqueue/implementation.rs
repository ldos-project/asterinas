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

// TODO(arthurp): Inline observer and consumer functions are called with the OQueue spinlock held
// (and interrupts disabled). This means that they are VERY limited in what they can do. Notably,
// they cannot produce to another OQueue. This needs to be relaxed and there needs to be a way for
// the inline handler to participate in the `try_`/`can_` machinery to enable async production
// while inline handlers are attached.

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};
use core::{
    any::TypeId,
    marker::PhantomData,
    ops::DerefMut,
    sync::atomic::{AtomicUsize, Ordering},
};

use log::warn;
use slotmap::{SlotMap, new_key_type};
use snafu::ensure;

mod stream;

use self::stream::{
    ErasedHandler, ErasedOutput, ObservationStream, ReadPosition, ReaderKey, ReaderMode,
    TypedObservationStream,
};
use crate::{
    orpc::{
        oqueue::{
            Cursor, ElementDescriptor, InlineStrongObserver, LifetimelessElementDescriptor,
            OQueueError, ObservationQuery, ResourceUnavailableSnafu,
            single_thread_ring_buffer::RingBuffer,
        },
        path::Path,
    },
    sync::{LocalIrqDisabled, SpinLock, WaitQueue, WakerKey},
    task::{Task, scheduler::switch_to_target},
};

new_key_type! {
    /// A unique identifier for an observer ring buffer.
    pub struct ObserverKey;
    /// A unique identifier for an inline strong observer.
    pub struct InlineObserverKey;
    /// A unique identifier for a wake hook.
    pub struct WakeHookKey;
    struct StreamKey;
}

/// The OQueue event a [`WakeHook`] waits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WakeEvent {
    /// A value was produced, so observers and consumers may have something to read.
    Produced,
    /// The last full buffer that blocked producers gained space, so a produce would not block.
    SpaceFreed,
}

/// A hook registered with `on_ready` on [`ValueProducer`](super::ValueProducer),
/// [`Consumer`](super::Consumer), or [`StrongObserver`](super::StrongObserver). The hook stays
/// registered until this is dropped.
pub struct WakeHook {
    oqueue: Weak<dyn UntypedOQueueImplementation>,
    event: WakeEvent,
    key: WakeHookKey,
}

impl WakeHook {
    /// Registers `hook` to run on each `event` of `oqueue`.
    ///
    /// The hook runs in atomic context, so it must not sleep or add or remove wake hooks on this
    /// OQueue.
    pub(super) fn register(
        oqueue: Arc<dyn UntypedOQueueImplementation>,
        event: WakeEvent,
        hook: Box<dyn Fn() + Send>,
    ) -> Self {
        let key = oqueue.add_wake_hook(event, hook);
        Self {
            oqueue: Arc::downgrade(&oqueue),
            event,
            key,
        }
    }
}

impl Drop for WakeHook {
    fn drop(&mut self) {
        // A dropped OQueue took its hooks with it, so there is nothing to unregister.
        if let Some(oqueue) = self.oqueue.upgrade() {
            oqueue.remove_wake_hook(self.event, self.key);
        }
    }
}

/// The registered [`WakeHook`]s of an OQueue, one list per [`WakeEvent`] so that a wake-up only
/// visits the hooks waiting on that event.
#[derive(Default)]
struct WakeHooks {
    produced: SlotMap<WakeHookKey, Box<dyn Fn() + Send>>,
    space_freed: SlotMap<WakeHookKey, Box<dyn Fn() + Send>>,
}

impl WakeHooks {
    /// Returns the hooks waiting on `event`.
    fn for_event(&mut self, event: WakeEvent) -> &mut SlotMap<WakeHookKey, Box<dyn Fn() + Send>> {
        match event {
            WakeEvent::Produced => &mut self.produced,
            WakeEvent::SpaceFreed => &mut self.space_freed,
        }
    }
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
pub(crate) struct OQueueImplementation<D: ElementDescriptor> {
    // TODO(arthurp): A number of methods perform allocation while this lock is held. Do we actually
    // want to disable IRQs?
    inner: SpinLock<OQueueInner<D>, LocalIrqDisabled>,
    /// The size to use for the consumer and strong-observer ring-buffers.
    len: usize,
    supports_consume: bool,
    path: Option<Path>,
    /// Wait queue for observers. Awoken when there are elements to observe.
    observe_wait_queue: WaitQueue,
    /// Wait queue for consumers. Awoken when there are elements to consume. This is
    /// [switched-to](`WaitQueue::switch_to_one`), so it is separate from `observe_wait_queue`.
    pub(super) consume_wait_queue: WaitQueue,
    /// The last task to attach to this OQueue as a consumer. This is weak so as not to keep the
    /// [`Task`] alive if no other consumer appears for a long period of time.
    last_consumer: SpinLock<Option<Weak<Task>>, LocalIrqDisabled>,
    /// Wait queue for producers. Awoken when there is space for more elements.
    produce_wait_queue: WaitQueue,
    /// Hooks run alongside the waiter wake-ups, one list per event.
    wake_hooks: SpinLock<WakeHooks, LocalIrqDisabled>,
    /// The number of full buffers that block producers: the consumer ring and non-revocable
    /// strong-observer rings. A produce does not block iff this is 0.
    num_full_buffers: AtomicUsize,
}

impl<D: ElementDescriptor + 'static> OQueueImplementation<D> {
    /// Create a new OQueue.
    ///
    /// * `len` is the ring buffer length used for consumers and strong-observers.
    /// * `supports_consume` specifies the attachment it allows later.
    /// * `path` is the queue's path, or `None` for an anonymous queue.
    pub(crate) fn new(mut len: usize, supports_consume: bool, path: Option<Path>) -> Self {
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
                streams: Default::default(),
                observers: Default::default(),
                inline_strong_observers: Default::default(),
                inline_consumer: None,
            }),
            len,
            supports_consume,
            path,
            observe_wait_queue: WaitQueue::new(),
            consume_wait_queue: WaitQueue::new(),
            last_consumer: SpinLock::new(None),
            produce_wait_queue: WaitQueue::new(),
            wake_hooks: SpinLock::new(WakeHooks::default()),
            num_full_buffers: AtomicUsize::new(0),
        }
    }

    /// Return the path associated with this OQueue, or `None` for anonymous queues.
    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_ref()
    }

    /// Detach a consumer. This will free the consumer ring buffer if there are no consumers left.
    pub(super) fn detach_consumer(self: &Arc<Self>) {
        let mut inner = self.inner.lock();
        inner.n_consumers -= 1;
        let mut freed = false;
        if inner.n_consumers == 0 {
            let ring_buffer = inner.consumer_ring_buffer.take();
            freed = self.unfull(ring_buffer.is_some_and(|rb| !rb.can_produce()));
        }
        drop(inner);
        if freed {
            self.wake_space_freed();
        }
    }

    pub(super) fn detach_inline_consumer(&self) {
        let mut inner = self.inner.lock();
        inner.inline_consumer = None;
    }

    /// Attaches a reader to a new query-backed stream. Query sharing can later reuse a stream.
    fn attach_observer<U: Copy + Send + 'static>(
        &self,
        query: ObservationQuery<D, U>,
        len: usize,
        mode: ReaderMode,
    ) -> Result<ObserverKey, OQueueError> {
        let mut stream = TypedObservationStream::new(query, len);
        let reader = stream
            .attach_reader(mode)
            .map_err(|_| ResourceUnavailableSnafu.build())?;
        let mut inner = self.inner.lock();
        let key = inner.streams.insert(Box::new(stream));
        Ok(inner.observers.insert((key, reader)))
    }

    /// Attach a strong observer.
    pub(super) fn attach_strong_observer<U>(
        self: &Arc<Self>,
        query: ObservationQuery<D, U>,
    ) -> Result<super::StrongObserver<U>, OQueueError>
    where
        U: Copy + Send + 'static,
    {
        self.attach_strong_observer_inner(query, ReaderMode::Strong)
    }

    /// Attach a revocable strong observer that is dropped, rather than blocking producers, when it
    /// falls too far behind.
    pub(super) fn attach_revocable_strong_observer<U>(
        self: &Arc<Self>,
        query: ObservationQuery<D, U>,
    ) -> Result<super::StrongObserver<U>, OQueueError>
    where
        U: Copy + Send + 'static,
    {
        self.attach_strong_observer_inner(query, ReaderMode::RevocableStrong)
    }

    fn attach_strong_observer_inner<U>(
        self: &Arc<Self>,
        query: ObservationQuery<D, U>,
        mode: ReaderMode,
    ) -> Result<super::StrongObserver<U>, OQueueError>
    where
        U: Copy + Send + 'static,
    {
        let observer_key = self.attach_observer(query, self.len, mode)?;
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
        f: impl for<'a> Fn(&'a D::Element<'a>) + Send + 'static,
    ) -> Result<InlineStrongObserver, OQueueError> {
        let mut inner = self.inner.lock();
        let key = inner
            .inline_strong_observers
            .insert(InlineObserver::Direct(Box::new(f)));
        Ok(InlineStrongObserver {
            oqueue: self.clone(),
            inline_observer_id: key,
            _phantom: PhantomData,
        })
    }

    /// Converts a strong observer to inline delivery through the stream's erased interface.
    /// Buffered values are drained for this reader; other readers retain their positions.
    pub(super) fn convert_strong_observer_to_inline<U: Send + Copy + 'static>(
        this: &dyn UntypedOQueueImplementation,
        observer_id: ObserverKey,
        handler_fn: Box<dyn Fn(&U) + Send + 'static>,
    ) -> Result<InlineObserverKey, OQueueError> {
        this.make_observer_inline(observer_id, Box::new(handler_fn))
    }

    /// Attach a weak observer.
    pub(super) fn attach_weak_observer<U>(
        self: &Arc<Self>,
        history_len: usize,
        query: ObservationQuery<D, U>,
    ) -> Result<super::WeakObserver<U>, OQueueError>
    where
        U: Copy + Send + 'static,
    {
        let observer_key = self.attach_observer(query, history_len, ReaderMode::Weak)?;
        Ok(super::WeakObserver {
            oqueue: self.clone(),
            observer_id: observer_key,
            last_observed: Cursor(0).into(),
            _phantom: PhantomData,
        })
    }

    /// Create a new reference to `self` without its OQueue kind.
    pub(super) fn as_any_oqueue(self: &Arc<Self>) -> super::GenericAnyOQueueRef<D> {
        super::GenericAnyOQueueRef {
            inner: self.clone(),
        }
    }
}

impl<D: ElementDescriptor + 'static> OQueueImplementation<D> {
    /// Revokes slow readers and checks all required buffers for space.
    fn prepare_produce(&self, inner: &mut OQueueInner<D>) -> bool {
        for stream in inner.streams.values_mut() {
            stream.revoke_full_readers();
        }
        let ready = inner.streams.values().all(|s| !s.blocks_production())
            && inner.consumer_ring_buffer.iter().all(|r| r.can_produce());
        debug_assert_eq!(ready, self.can_produce(), "num_full_buffers is out of sync");
        ready
    }

    /// True if try_produce into self is *expected* to succeed. It may still fail if there is a
    /// concurrent producer.
    pub(super) fn can_produce(&self) -> bool {
        self.num_full_buffers.load(Ordering::Acquire) == 0
    }

    /// Produce a value by reference, blocking until it completes.
    pub(super) fn produce_ref<'a>(&self, v: &'a D::Element<'a>) {
        self.produce_wait_queue.wait_until(|| {
            if self.try_produce_ref(v) {
                Some(())
            } else {
                None
            }
        })
    }

    /// Attempt to produce a value by reference. This executes all the queries on `v` and places the
    /// query results into the appropriate observation ring buffers.
    pub(super) fn try_produce_ref<'a>(&self, v: &'a D::Element<'a>) -> bool {
        let mut inner = self.inner.lock();
        assert!(inner.consumer_ring_buffer.is_none());
        if self.prepare_produce(&mut inner) {
            let newly_full = inner.produce_observations(v);
            self.num_full_buffers
                .fetch_add(newly_full, Ordering::Relaxed);
            drop(inner);

            self.wake_after_produce();

            true
        } else {
            false
        }
    }

    /// Wake observers and consumers after a successful produce.
    fn wake_after_produce(&self) {
        self.observe_wait_queue.wake_all();
        self.run_wake_hooks(WakeEvent::Produced);

        // Try to wake a consumer which is blocked on this OQueue. If no OQueue was awoken and there
        // was some task which consumed on this OQueue before, wake that.
        if !self.consume_wait_queue.switch_to_one() {
            let mut last_consumer = self.last_consumer.lock();
            if let Some(task) = last_consumer.as_ref().and_then(Weak::upgrade) {
                drop(last_consumer);

                switch_to_target(task);
            } else {
                *last_consumer = None;
            }
        }
    }

    /// Updates `num_full_buffers` after a counted buffer changed or was removed, where `was_full`
    /// says whether that buffer had been full and now no longer is. Call this with `inner` held.
    ///
    /// Returns whether no full buffer is left, in which case the caller must call
    /// [`Self::wake_space_freed`] once `inner` is dropped.
    fn unfull(&self, was_full: bool) -> bool {
        if !was_full {
            return false;
        }
        let previous = self.num_full_buffers.fetch_sub(1, Ordering::Release);
        previous == 1
    }

    /// Wakes blocked producers and runs the [`WakeEvent::SpaceFreed`] hooks.
    fn wake_space_freed(&self) {
        self.produce_wait_queue.wake_all();
        self.run_wake_hooks(WakeEvent::SpaceFreed);
    }

    /// Runs the hooks registered for `event`.
    fn run_wake_hooks(&self, event: WakeEvent) {
        for hook in self.wake_hooks.lock().for_event(event).values() {
            hook();
        }
    }

    /// Attach an producer expecting references to the OQueue if it has no consumers.
    pub(super) fn attach_ref_producer(
        self: &Arc<Self>,
    ) -> Result<super::GenericRefProducer<D>, OQueueError> {
        if self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        Ok(super::GenericRefProducer {
            oqueue: self.clone(),
        })
    }

    /// True if at least one strong or weak observer is currently attached.
    pub(super) fn has_observers(&self) -> bool {
        !self.inner.lock().observers.is_empty()
    }
}

// This impl block is specific to `LifetimelessElementDescriptor<T>` (rather than generic over any
// `D: ElementDescriptor`) because producing/consuming by value requires an owned (`'static`)
// message.
impl<T: Send + 'static> OQueueImplementation<LifetimelessElementDescriptor<T>> {
    /// Produce into the OQueue. Blocking until space is available
    pub(super) fn produce(&self, v: T) {
        let mut v = v;
        loop {
            if let Err(ret) = self.try_produce(v) {
                v = ret;
                // TODO: PERFORMANCE: Using can_produce here and try_produce above requires 2
                // acquires.
                self.produce_wait_queue
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
        if self.prepare_produce(&mut inner) {
            let mut newly_full = inner.produce_observations(&v);
            assert!(inner.consumer_ring_buffer.is_none() || inner.inline_consumer.is_none());
            if let Some(ring_buffer) = &mut inner.consumer_ring_buffer {
                let v = ring_buffer.try_produce(v);
                assert!(v.is_none());
                if !ring_buffer.can_produce() {
                    newly_full += 1;
                }
            } else if let Some(consumer) = &inner.inline_consumer {
                consumer(v);
            } else {
                // No consumer attached, so `v` has nowhere to land; warn instead of dropping it
                // silently.
                warn!(
                    "Produced a value into a ConsumableOQueue with no consumer attached; the value is dropped"
                );
            }
            self.num_full_buffers
                .fetch_add(newly_full, Ordering::Relaxed);
            drop(inner);

            self.wake_after_produce();

            Ok(())
        } else {
            Err(v)
        }
    }

    /// Consume a value, blocking until it completes.
    pub(super) fn consume(&self) -> T {
        self.consume_wait_queue.wait_until(|| self.try_consume())
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
        let ring_buffer = inner
            .consumer_ring_buffer
            .as_mut()
            .expect("consume not supported");
        let was_full = !ring_buffer.can_produce();
        let ret = ring_buffer.try_consume();
        let freed = ret.is_some() && self.unfull(was_full);
        drop(inner);
        *self.last_consumer.lock().deref_mut() =
            Task::current().map(|t| Arc::downgrade(&t.cloned()));
        if freed {
            self.wake_space_freed();
        }
        ret
    }

    /// Attach a by-value producer to the OQueue if this supports consumers.
    pub(super) fn attach_value_producer(
        self: &Arc<Self>,
    ) -> Result<super::ValueProducer<T>, OQueueError> {
        if !self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        Ok(super::ValueProducer {
            oqueue: self.clone(),
        })
    }

    /// Attach a consumer to the OQueue if this does not support consumers.
    pub(super) fn attach_consumer(self: &Arc<Self>) -> Result<super::Consumer<T>, OQueueError> {
        if !self.supports_consume {
            return super::UnsupportedSnafu.fail();
        }
        let mut inner = self.inner.lock();
        if inner.consumer_ring_buffer.is_none() {
            let mut ring_buffer =
                RingBuffer::new(self.len).map_err(|_| ResourceUnavailableSnafu.build())?;
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
    ) -> Result<(), OQueueError> {
        let inline_consumer: Box<dyn Fn(T) + Send + 'static> = Box::new(f);
        let mut inner = self.inner.lock();
        ensure!(inner.inline_consumer.is_none(), ResourceUnavailableSnafu);
        inner.inline_consumer = Some(inline_consumer);
        Ok(())
    }

    /// True if at least one consumer is currently attached to this OQueue.
    pub(super) fn has_consumers(&self) -> bool {
        let inner = self.inner.lock();
        inner.n_consumers > 0 || inner.inline_consumer.is_some()
    }
}

/// The part of [`OQueueImplementation`] which is protected by a lock.
struct OQueueInner<D: ElementDescriptor> {
    /// The ring buffer used for consumers. Using `'static` is safe here because by construction any
    /// OQueue that supports consumers has an element type with `Send + 'static` (see
    /// ConsumableOQueueRef and the impl for GenericAnyOQueueRef).
    consumer_ring_buffer: Option<RingBuffer<D::Element<'static>>>,
    /// The number of attached consumers. This is required to detect when the `consumer_ring_buffer`
    /// can be discarded.
    n_consumers: usize,
    /// A query and result ring, independently of the handles attached to it.
    streams: SlotMap<StreamKey, Box<dyn ObservationStream<D>>>,
    observers: SlotMap<ObserverKey, (StreamKey, ReaderKey)>,
    // TODO: PERFORMANCE: There will be a bunch of cases where there is *only* a consumer ring
    // buffer, or *only* a single observer ring. It would be nice if either could be inlined. As of
    // now, the consumer is always inlined even when it's not in use, and the observer never is.
    // TODO: PERFORMANCE: This needs a way to share ring buffers when multiple consumers use the
    // same query.
    /// Inline strong observers of the message type. These will be called during production.
    inline_strong_observers: SlotMap<InlineObserverKey, InlineObserver<D>>,
    /// The inline consumer if there is one.
    ///
    /// This is only ever populated when `MD = LifetimelessElementDescriptor<T>` (see
    /// `OQueueImplementation<LifetimelessElementDescriptor<T>>`'s by-value impl block), since only an
    /// owned, `'static` message can be consumed by value.
    inline_consumer: Option<Box<dyn Fn(D::Element<'static>) + Send>>,
}

type InlineHandler<D> = Box<dyn for<'a> Fn(&'a <D as ElementDescriptor>::Element<'a>) + Send>;

enum InlineObserver<D: ElementDescriptor> {
    Direct(InlineHandler<D>),
    Stream(StreamKey, ReaderKey),
}

impl<D: ElementDescriptor> OQueueInner<D> {
    /// Returns how many previously writable streams became full.
    fn produce_observations<'a>(&mut self, value: &'a D::Element<'a>) -> usize {
        // Every counted ring has room here, so a counted ring that is full afterwards has just
        // become full.
        let mut newly_full = 0;
        for stream in self.streams.values_mut() {
            stream.produce(value);
            newly_full += usize::from(stream.blocks_production());
        }
        for observer in self.inline_strong_observers.values() {
            if let InlineObserver::Direct(handler_fn) = observer {
                handler_fn(value);
            }
        }
        newly_full
    }

    fn observer(&self, key: ObserverKey) -> (&dyn ObservationStream<D>, ReaderKey) {
        let (stream, reader) = self.observers[key];
        (self.streams[stream].as_ref(), reader)
    }

    fn observer_mut(&mut self, key: ObserverKey) -> (&mut dyn ObservationStream<D>, ReaderKey) {
        let (stream, reader) = self.observers[key];
        (self.streams[stream].as_mut(), reader)
    }

    /// Returns whether removing this reader freed a blocking ring.
    fn detach_reader(&mut self, stream: StreamKey, reader: ReaderKey) -> bool {
        let stream_ref = &mut self.streams[stream];
        let was_full = stream_ref.blocks_production();
        let empty = stream_ref.detach_reader(reader);
        let freed = was_full && !stream_ref.blocks_production();
        if empty {
            self.streams.remove(stream);
        }
        freed
    }
}

/// A dyn-compatible trait to type erase `D` from [`OQueueImplementation<D>`]. The operations here
/// conceptually take the observed type `U` as a type parameter, but this is not possible in a
/// dyn-compatible trait.
pub(super) trait UntypedOQueueImplementation: Sync + Send {
    /// Release any resources held by observer with the given ID. Using that `observer_id` again may
    /// produce UB.
    fn detach_observer(&self, observer_id: ObserverKey);

    /// Release any resources held by inline observer with the given key.
    fn detach_inline_strong_observer(&self, inline_observer_id: InlineObserverKey);

    fn can_strong_observe(&self, observer_id: ObserverKey) -> bool;

    /// Enqueue a waker for an observer into the wait queue. This is used to implement `Blocker` for
    /// `StrongObserver`.
    fn enqueue_observe_waker(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey;

    /// Remove a waker for an observer from the wait queue. This is used to implement `Blocker` for
    /// `StrongObserver`.
    fn remove_observe_waker(&self, key: WakerKey);

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
    ) -> Result<bool, OQueueError>;

    /// A blocking version of [`UntypedOQueueImplementation::try_strong_observe_into`]. All the safety
    /// requirements on that apply here, except that this will never return `Ok` without filling
    /// `dest`, since it blocks waiting for a value.
    unsafe fn strong_observe_into(
        &self,
        observer_id: ObserverKey,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), OQueueError>;

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
    ) -> Result<bool, OQueueError>;

    /// Converts one reader without disturbing the stream's other readers.
    fn make_observer_inline(
        &self,
        observer_id: ObserverKey,
        handler: ErasedHandler,
    ) -> Result<InlineObserverKey, OQueueError>;

    /// Wait until new values are available for the specific observer.
    fn wait(&self, observer_id: ObserverKey, cursor: Cursor);

    /// Get the most recent cursor which will return a value.
    fn newest_cursor(&self, observer_id: ObserverKey) -> Cursor;

    /// Get the oldest cursor which will return a value.
    fn oldest_cursor(&self, observer_id: ObserverKey) -> Cursor;

    /// Register a hook to run where the OQueue wakes its waiters. This is used to implement
    /// [`WakeHook`].
    fn add_wake_hook(&self, event: WakeEvent, hook: Box<dyn Fn() + Send>) -> WakeHookKey;

    /// Unregister a hook added by [`Self::add_wake_hook`] for `event`.
    fn remove_wake_hook(&self, event: WakeEvent, key: WakeHookKey);
}

impl<D: ElementDescriptor + 'static> UntypedOQueueImplementation for OQueueImplementation<D> {
    fn detach_observer(&self, observer_id: ObserverKey) {
        let mut inner = self.inner.lock();
        let freed = inner
            .observers
            .remove(observer_id)
            .is_some_and(|(stream, reader)| inner.detach_reader(stream, reader));
        let freed = self.unfull(freed);
        drop(inner);
        if freed {
            self.wake_space_freed();
        }
    }

    fn detach_inline_strong_observer(&self, inline_observer_id: InlineObserverKey) {
        let mut inner = self.inner.lock();
        if let Some(InlineObserver::Stream(stream, reader)) =
            inner.inline_strong_observers.remove(inline_observer_id)
        {
            inner.detach_reader(stream, reader);
        }
    }

    unsafe fn try_strong_observe_into(
        &self,
        observer_id: ObserverKey,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<bool, OQueueError> {
        let mut inner = self.inner.lock();
        let (stream, reader) = inner.observer_mut(observer_id);
        let was_full = stream.blocks_production();
        // SAFETY: The caller supplies storage for this observer's output type.
        let ret = unsafe {
            stream.read_into(reader, ReadPosition::Next, ErasedOutput::new(dest, type_id))
        }?;
        let freed = self.unfull(was_full && !stream.blocks_production());
        drop(inner);
        if freed {
            self.wake_space_freed();
        }
        Ok(ret)
    }

    fn make_observer_inline(
        &self,
        observer_id: ObserverKey,
        handler: ErasedHandler,
    ) -> Result<InlineObserverKey, OQueueError> {
        let mut inner = self.inner.lock();
        let (stream, reader) = inner
            .observers
            .remove(observer_id)
            .expect("observer is attached");
        let stream_ref = &mut inner.streams[stream];
        let was_full = stream_ref.blocks_production();
        stream_ref.make_inline(reader, handler);
        let freed = self.unfull(was_full && !stream_ref.blocks_production());
        let key = inner
            .inline_strong_observers
            .insert(InlineObserver::Stream(stream, reader));
        drop(inner);
        if freed {
            self.wake_space_freed();
        }
        Ok(key)
    }

    unsafe fn strong_observe_into(
        &self,
        observer_id: ObserverKey,
        _type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), OQueueError> {
        self.observe_wait_queue.wait_until(|| {
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
    ) -> Result<bool, OQueueError> {
        let mut inner = self.inner.lock();
        let (stream, reader) = inner.observer_mut(observer_id);
        // SAFETY: The caller supplies storage for this observer's output type.
        unsafe {
            stream.read_into(
                reader,
                ReadPosition::At(cursor),
                ErasedOutput::new(dest, _type_id),
            )
        }
    }

    fn wait(&self, observer_id: ObserverKey, cursor: Cursor) {
        self.observe_wait_queue.wait_until(|| {
            if self.newest_cursor(observer_id) > cursor {
                Some(())
            } else {
                None
            }
        })
    }

    fn newest_cursor(&self, observer_id: ObserverKey) -> Cursor {
        self.inner.lock().observer(observer_id).0.cursor_bounds().1
    }

    fn oldest_cursor(&self, observer_id: ObserverKey) -> Cursor {
        self.inner.lock().observer(observer_id).0.cursor_bounds().0
    }

    fn can_strong_observe(&self, observer_id: ObserverKey) -> bool {
        let inner = self.inner.lock();
        let (stream, reader) = inner.observer(observer_id);
        stream.can_get(reader)
    }

    fn enqueue_observe_waker(&self, waker: &Arc<crate::sync::Waker>) -> WakerKey {
        self.observe_wait_queue.enqueue(waker.clone())
    }

    fn remove_observe_waker(&self, key: WakerKey) {
        self.observe_wait_queue.remove(key);
    }

    fn add_wake_hook(&self, event: WakeEvent, hook: Box<dyn Fn() + Send>) -> WakeHookKey {
        self.wake_hooks.lock().for_event(event).insert(hook)
    }

    fn remove_wake_hook(&self, event: WakeEvent, key: WakeHookKey) {
        self.wake_hooks.lock().for_event(event).remove(key);
    }
}

#[cfg(ktest)]
mod test {
    use super::{
        super::{StrongObserver, WeakObserver},
        *,
    };
    use crate::prelude::*;

    type TestQueue = OQueueImplementation<LifetimelessElementDescriptor<u32>>;

    fn shared_observer(queue: &Arc<TestQueue>, source: ObserverKey) -> StrongObserver<u32> {
        let mut inner = queue.inner.lock();
        let stream = inner.observers[source].0;
        let reader = inner.streams[stream]
            .attach_reader(ReaderMode::Strong)
            .unwrap();
        StrongObserver {
            oqueue: queue.clone(),
            observer_id: inner.observers.insert((stream, reader)),
            convert_to_inline: TestQueue::convert_strong_observer_to_inline,
            _phantom: PhantomData,
        }
    }

    #[ktest]
    fn shared_readers_keep_independent_positions_and_inline_delivery() {
        let queue = Arc::new(TestQueue::new(2, false, None));
        let calls = Arc::new(AtomicUsize::new(0));
        let first = queue
            .attach_strong_observer(ObservationQuery::new_filter({
                let calls = calls.clone();
                move |value: &u32| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    value.is_multiple_of(2).then_some(*value)
                }
            }))
            .unwrap();
        let second = shared_observer(&queue, first.observer_id);
        let weak = {
            let mut inner = queue.inner.lock();
            let stream = inner.observers[first.observer_id].0;
            let reader = inner.streams[stream]
                .attach_reader(ReaderMode::Weak)
                .unwrap();
            WeakObserver::<u32> {
                oqueue: queue.clone(),
                observer_id: inner.observers.insert((stream, reader)),
                last_observed: Cursor(0).into(),
                _phantom: PhantomData,
            }
        };
        assert!(queue.try_produce_ref(&1));
        assert_eq!(first.try_strong_observe().unwrap(), None);
        assert!(queue.try_produce_ref(&2));
        let observed: Arc<SpinLock<Vec<u32>>> = Arc::new(SpinLock::new(Vec::new()));
        let inline = first
            .strong_observe_inline({
                let observed = observed.clone();
                move |value| observed.lock().push(*value)
            })
            .unwrap();
        assert!(!queue.can_produce());
        assert_eq!(second.try_strong_observe().unwrap(), Some(2));
        assert!(queue.can_produce());
        // Reuse the converted reader's head without changing the second reader's position.
        let third = shared_observer(&queue, second.observer_id);
        assert!(queue.try_produce_ref(&4));
        assert_eq!(second.try_strong_observe().unwrap(), Some(4));
        assert!(!queue.can_produce());
        drop(third);
        assert!(queue.can_produce());
        drop(inline);
        assert!(queue.try_produce_ref(&6));
        assert_eq!(second.try_strong_observe().unwrap(), Some(6));
        assert_eq!(*observed.lock(), [2, 4]);
        assert_eq!(weak.weak_observe_recent(2).unwrap(), [Some(4), Some(6)]);
        drop(second);
        assert!(queue.try_produce_ref(&8));
        assert_eq!(weak.weak_observe_recent(2).unwrap(), [Some(6), Some(8)]);
        assert_eq!(calls.load(Ordering::Relaxed), 5);
    }

    #[ktest]
    fn revoking_shared_reader_preserves_other_readers() {
        let queue = Arc::new(TestQueue::new(2, false, None));
        let slow = queue
            .attach_revocable_strong_observer(ObservationQuery::identity())
            .unwrap();
        let fast = shared_observer(&queue, slow.observer_id);
        assert!(queue.try_produce_ref(&1));
        assert_eq!(fast.try_strong_observe().unwrap(), Some(1));
        assert!(queue.can_produce());
        assert!(queue.try_produce_ref(&2));
        assert!(matches!(
            slow.try_strong_observe(),
            Err(OQueueError::Revoked { .. })
        ));
        let observed = Arc::new(AtomicUsize::new(0));
        let _inline = slow
            .strong_observe_inline({
                let observed = observed.clone();
                move |value| {
                    observed.store(*value as usize, Ordering::Relaxed);
                }
            })
            .unwrap();
        assert_eq!(observed.load(Ordering::Relaxed), 0);
        assert!(!queue.can_produce());
        assert_eq!(fast.try_strong_observe().unwrap(), Some(2));
        assert!(queue.try_produce_ref(&3));
        assert_eq!(fast.try_strong_observe().unwrap(), Some(3));
        assert_eq!(observed.load(Ordering::Relaxed), 3);
    }

    #[ktest]
    fn owned_values_are_dropped_once_after_consume_and_detach() {
        struct Value(Arc<AtomicUsize>);
        impl Drop for Value {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let queue = Arc::new(
            OQueueImplementation::<LifetimelessElementDescriptor<Value>>::new(4, true, None),
        );
        let consumer = queue.attach_consumer().unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        queue.produce(Value(drops.clone()));
        queue.produce(Value(drops.clone()));
        drop(consumer.try_consume().unwrap());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        drop(consumer);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }

    /// Producing takes `last_consumer`, and some OQueues are produced from IRQ context (e.g.
    /// `scheduler.events` wakeups from a block I/O completion). If the lock left IRQs enabled, a
    /// handler producing to the same OQueue could interrupt a holder and spin on it forever.
    #[ktest]
    fn last_consumer_lock_disables_local_irqs() {
        let queue = OQueueImplementation::<LifetimelessElementDescriptor<u32>>::new(4, true, None);
        assert!(crate::arch::irq::is_local_enabled());

        let guard = queue.last_consumer.lock();
        assert!(!crate::arch::irq::is_local_enabled());
        drop(guard);

        assert!(crate::arch::irq::is_local_enabled());
    }
}
