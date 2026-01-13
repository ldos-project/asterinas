// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{any::TypeId, marker::PhantomData};

use static_assertions::assert_obj_safe;

use crate::{
    orpc::oqueue::{
        Cursor, HandleAccessError, ObservationQuery, single_thread_ring_buffer::RingBuffer,
    },
    sync::{SpinLock, WaitQueue},
};

/// The underlying implementation of OQueues. This is used within an `Arc` to reference the OQueue internally. Externally, code will use [`super::OQueueRef`] and similar.
pub(crate) struct OQueueImplementation<T> {
    inner: SpinLock<OQueueInner<T>>,
    /// The size to use for the consumer and strong-observer ring-buffers.
    len: usize,
    is_communication_oqueue: bool,
    put_wait_queue: WaitQueue,
    read_wait_queue: WaitQueue,
}

impl<T> OQueueImplementation<T> {
    pub(crate) fn new(len: usize, is_communication_oqueue: bool) -> Self {
        Self {
            inner: SpinLock::new(OQueueInner {
                consumer_ring_buffer: Default::default(),
                n_consumers: 0,
                observer_ring_buffers: Default::default(),
            }),
            len,
            is_communication_oqueue,
            put_wait_queue: WaitQueue::new(),
            read_wait_queue: WaitQueue::new(),
        }
    }

    pub(super) fn detach_consumer(self: &Arc<Self>) {
        let mut data = self.inner.lock();
        data.n_consumers -= 1;
        if data.n_consumers == 0 {
            data.consumer_ring_buffer = None;
        }
    }
}

/// The part of [`OQueueImplementation`] which is protected by a lock.
struct OQueueInner<T> {
    /// The ring buffer used for consumers.
    consumer_ring_buffer: Option<RingBuffer>,
    /// The number of attached consumers. This is required to detect when the `consumer_ring_buffer`
    /// can be discarded.
    n_consumers: usize,
    /// The ring buffers used for observers.
    observer_ring_buffers: Vec<Option<ObservationRingBuffer<T>>>,
    // TODO: PERFORMANCE: There will be a bunch of cases where there is *only* a consumer ring
    // buffer, or *only* a single observer ring. It would be nice if either could be inlined. As of
    // now, the consumer is always inlined even when it's not in use, and the observer never is.
    // TODO: PERFORMANCE: This needs a way to share ring buffers when multiple consumers use the
    // same query.
}

impl<T> OQueueInner<T> {
    fn can_produce(&self) -> bool {
        self.observer_ring_buffers
            .iter()
            .all(|r| r.as_ref().is_none_or(|r| r.ring_buffer.can_produce()))
            && self.consumer_ring_buffer.iter().all(|r| r.can_produce())
    }
}

/// A partially type erased [`ObservationQuery`] with `U` (the observed query output type) erased,
/// but `T` (the message type) retained statically. This is used in the OQueue machinery to dispatch
/// to the query and produce it's result into a [`RingBuffer`].
trait ErasedObservationQuery<T: ?Sized>: Send {
    /// Call the query and then produce the value into the provided ring buffer. The ring buffer
    /// must have a type matching the result of the query (`U` below). Returns false if the value
    /// could not be produced into `ring_buffer`.
    fn call_into(&self, v: &T, ring_buffer: &mut RingBuffer) -> bool;
}

impl<T: ?Sized, U: 'static> ErasedObservationQuery<T> for ObservationQuery<T, U> {
    fn call_into(&self, v: &T, ring_buffer: &mut RingBuffer) -> bool {
        if let Some(v) = self.call(v) {
            ring_buffer.try_produce(v).is_none()
        } else {
            true
        }
    }
}

struct ObservationRingBuffer<T> {
    // TODO: PERFORMANCE: Replace the dyn ref and optimize based on the known structure of
    // ObservationQuery. This should remove a level of pointer indirection.
    /// The query to filter `T` and place the result into the ring buffer.
    query: Box<dyn ErasedObservationQuery<T>>,
    /// Function to extract a value from the ring buffer with strong observer semantics. This
    /// indirection is needed since the type `U` in the ring buffer is not statically known.
    try_strong_observe_into: fn(&mut RingBuffer, dest: *mut ()) -> bool,
    /// Function to extract a value from the ring buffer. This indirection is needed since the
    /// type `U` in the ring buffer is not statically known.
    weak_observe_into: fn(&mut RingBuffer, Cursor, dest: *mut ()) -> bool,
    ring_buffer: RingBuffer,
}

impl<T: Send + 'static> OQueueImplementation<T> {
    fn can_produce(&self) -> bool {
        self.inner.lock().can_produce()
    }

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

    pub(super) fn try_produce(&self, v: T) -> Result<(), T> {
        let mut data = self.inner.lock();
        if data.can_produce() {
            for ObservationRingBuffer {
                query, ring_buffer, ..
            } in data
                .observer_ring_buffers
                .iter_mut()
                .filter_map(|v| v.as_mut())
            {
                query.call_into(&v, ring_buffer);
            }
            if let Some(ring_buffer) = &mut data.consumer_ring_buffer {
                let v = ring_buffer.try_produce(v);
                assert!(v.is_none());
            }
            self.read_wait_queue.wake_all();
            Ok(())
        } else {
            Err(v)
        }
    }

    pub(super) fn produce_ref(&self, v: &T) {
        self.put_wait_queue.wait_until(|| {
            if self.try_produce_ref(v) {
                Some(())
            } else {
                None
            }
        })
    }

    pub(super) fn try_produce_ref(&self, v: &T) -> bool {
        let mut data = self.inner.lock();
        assert!(data.consumer_ring_buffer.is_none());
        if data.can_produce() {
            for ObservationRingBuffer {
                query, ring_buffer, ..
            } in data
                .observer_ring_buffers
                .iter_mut()
                .filter_map(|v| v.as_mut())
            {
                query.call_into(&v, ring_buffer);
            }
            self.read_wait_queue.wake_all();
            true
        } else {
            false
        }
    }

    pub(super) fn consume(&self) -> T {
        self.read_wait_queue.wait_until(|| self.try_consume())
    }

    pub(super) fn try_consume(&self) -> Option<T> {
        let mut data = self.inner.lock();
        // SAFETY: The consumer ring buffer is never used for observation.
        let ret = unsafe {
            data.consumer_ring_buffer
                .as_mut()
                .expect("consume not supported")
                .try_consume()
        };
        if ret.is_some() {
            self.put_wait_queue.wake_one();
        }
        ret
    }

    pub(super) fn attach_communication_producer(
        self: &Arc<Self>,
    ) -> Result<super::CommunicationProducer<T>, super::AttachmentError> {
        if !self.is_communication_oqueue {
            return Err(super::AttachmentError::Unsupported);
        }
        Ok(super::CommunicationProducer {
            oqueue: self.clone(),
            _phantom: PhantomData,
        })
    }

    pub(super) fn attach_observation_producer(
        self: &Arc<Self>,
    ) -> Result<super::ObservationProducer<T>, super::AttachmentError> {
        if self.is_communication_oqueue {
            return Err(super::AttachmentError::Unsupported);
        }
        Ok(super::ObservationProducer {
            oqueue: self.clone(),
            _phantom: PhantomData,
        })
    }

    pub(super) fn attach_consumer(
        self: &Arc<Self>,
    ) -> Result<super::Consumer<T>, super::AttachmentError> {
        if !self.is_communication_oqueue {
            return Err(super::AttachmentError::Unsupported);
        }
        let mut data = self.inner.lock();
        if data.consumer_ring_buffer.is_none() {
            let mut ring_buffer = RingBuffer::new::<T>(self.len)?;
            let id = ring_buffer.new_strong_reader();
            assert_eq!(id, 0);
            data.consumer_ring_buffer = Some(ring_buffer);
        }
        data.n_consumers += 1;
        Ok(super::Consumer {
            oqueue: self.clone(),
            _phantom: PhantomData,
        })
    }

    fn new_observation_ring_buffer<U>(
        self: &Arc<Self>,
        query: ObservationQuery<T, U>,
        len: usize,
        is_strong: bool,
    ) -> Result<usize, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let mut data = self.inner.lock();
        let mut ring_buffer = RingBuffer::new::<U>(len)?;
        if is_strong {
            let id = ring_buffer.new_strong_reader();
            // We don't reuse ring buffers, so for now this is always 0.
            assert_eq!(id, 0);
        }
        // We never reuse ring_buffers, so the ID is discarded and always 0, but that will change.
        let ring_buffer = ObservationRingBuffer {
            query: Box::new(query),
            try_strong_observe_into: |r, d| {
                // TODO: Use a real head_id when we share ring buffers.
                let head_id = 0;
                if let Some(v) = r.try_strong_observe::<U>(head_id) {
                    unsafe {
                        (d as *mut U).write(v);
                    }
                    true
                } else {
                    false
                }
            },
            weak_observe_into: |r, c, d| {
                if let Some(v) = r.try_weak_observe::<U>(c) {
                    unsafe {
                        (d as *mut U).write(v);
                    }
                    true
                } else {
                    false
                }
            },
            ring_buffer,
        };
        // Reuse any slot that is None, otherwise extend the vec.
        let observer_id = if let Some((id, slot)) = data
            .observer_ring_buffers
            .iter_mut()
            .enumerate()
            .find(|(_, v)| v.is_none())
        {
            *slot = Some(ring_buffer);
            id
        } else {
            data.observer_ring_buffers.push(Some(ring_buffer));
            data.observer_ring_buffers.len() - 1
        };
        Ok(observer_id)
    }

    pub(super) fn attach_strong_observer<U>(
        self: &Arc<Self>,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::StrongObserver<U>, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let observer_id = self.new_observation_ring_buffer(query, self.len, true)?;
        Ok(super::StrongObserver {
            oqueue: self.clone(),
            observer_id,
            _phantom: PhantomData,
        })
    }

    pub(super) fn attach_weak_observer<U>(
        self: &Arc<Self>,
        history_len: usize,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::WeakObserver<U>, super::AttachmentError>
    where
        U: Copy + Send + 'static,
    {
        let observer_id = self.new_observation_ring_buffer(query, history_len, false)?;
        Ok(super::WeakObserver {
            oqueue: self.clone(),
            observer_id,
            last_observed: Cursor(0).into(),
            _phantom: PhantomData,
        })
    }

    pub(super) fn as_any_oqueue(self: &Arc<Self>) -> super::AnyOQueueRef<T> {
        super::AnyOQueueRef {
            inner: self.clone(),
        }
    }
}

/// A dyn-compatible trait to type erase `OQueueInner<T>`.
///
/// TODO(arthurp): PERFORMANCE: These methods could be lifted out of the [`OQueueInner`] and written
/// directly on the untyped representation, since they are not actually going to involve `T`. This
/// would avoid an indirect function call and allow inlining. However, that requires having a
/// type-erased type which shares the layout of the typed variant. For the time being, we will do
/// this the easy way.
pub(super) trait UntypedOQueueInner: Sync + Send {
    /// Release any resources held by observer with the given ID. Using that `observer_id` again may
    /// produce UB.
    fn detach_strong_observer(&self, observer_id: usize);

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
        observer_id: usize,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<bool, HandleAccessError>;

    /// A blocking version of [`UntypedOQueueInner::try_strong_observe_into`]. All the safety
    /// requirements on that apply here, except that this will never return `Ok` without filling
    /// `dest`, since it blocks waiting for a value.
    unsafe fn strong_observe_into(
        &self,
        observer_id: usize,
        type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), HandleAccessError>;

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
        observer_id: usize,
        type_id: TypeId,
        cursor: Cursor,
        dest: *mut (),
    ) -> Result<bool, HandleAccessError>;

    /// Wait until new values are available for the specific observer.
    fn wait(&self, observer_id: usize, cursor: Cursor);

    fn recent_cursor(&self, observer_id: usize) -> Cursor;

    fn old_cursor(&self, observer_id: usize) -> Cursor;
}

assert_obj_safe!(UntypedOQueueInner);

impl<T: Send> UntypedOQueueInner for OQueueImplementation<T> {
    unsafe fn try_strong_observe_into(
        &self,
        observer_id: usize,
        _type_id: TypeId,
        dest: *mut (),
    ) -> Result<bool, HandleAccessError> {
        let mut data = self.inner.lock();
        let ObservationRingBuffer {
            try_strong_observe_into,
            ring_buffer,
            ..
        } = data.observer_ring_buffers[observer_id]
            .as_mut()
            .expect("should only be called with an id returned from new_observation_ring_buffer");

        let ret = try_strong_observe_into(ring_buffer, dest);
        if ret {
            self.put_wait_queue.wake_one();
        }
        Ok(ret)
    }

    unsafe fn strong_observe_into(
        &self,
        observer_id: usize,
        _type_id: TypeId,
        dest: *mut (),
    ) -> Result<(), HandleAccessError> {
        self.read_wait_queue.wait_until(|| {
            // SAFETY: The requirements of try_strong_observe_into are the same as this function.
            let r = unsafe { self.try_strong_observe_into(observer_id, _type_id, dest) };
            if let Ok(false) = r { None } else { Some(r) }
        })?;
        Ok(())
    }

    unsafe fn weak_observe_into(
        &self,
        observer_id: usize,
        _type_id: TypeId,
        cursor: Cursor,
        dest: *mut (),
    ) -> Result<bool, HandleAccessError> {
        let mut data = self.inner.lock();
        let ObservationRingBuffer {
            weak_observe_into,
            ring_buffer,
            ..
        } = data.observer_ring_buffers[observer_id]
            .as_mut()
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        Ok(weak_observe_into(ring_buffer, cursor, dest))
    }

    fn wait(&self, observer_id: usize, cursor: Cursor) {
        self.read_wait_queue.wait_until(|| {
            if self.recent_cursor(observer_id) > cursor {
                Some(())
            } else {
                None
            }
        })
    }

    fn recent_cursor(&self, observer_id: usize) -> Cursor {
        let mut data = self.inner.lock();
        let ObservationRingBuffer { ring_buffer, .. } = data.observer_ring_buffers[observer_id]
            .as_mut()
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        ring_buffer.recent_cursor()
    }

    fn old_cursor(&self, observer_id: usize) -> Cursor {
        let mut data = self.inner.lock();
        let ObservationRingBuffer { ring_buffer, .. } = data.observer_ring_buffers[observer_id]
            .as_mut()
            .expect("should only be called with an id returned from new_observation_ring_buffer");
        ring_buffer.old_cursor()
    }

    fn detach_strong_observer(&self, observer_id: usize) {
        let mut data = self.inner.lock();
        data.observer_ring_buffers[observer_id] = None;
    }
}
