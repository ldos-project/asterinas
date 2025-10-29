// SPDX-License-Identifier: MPL-2.0
//! A simple implementation of a [`crate::oqueue::OQueue`] using a [`crate::sync::Mutex`]s. This is a baseline
//! implementation that supports all features, but is also slow in many cases.

use alloc::{borrow::ToOwned, format, sync::Weak};
use core::{
    any::{Any, type_name},
    cell::Cell,
    panic::{RefUnwindSafe, UnwindSafe},
};

use super::{
    super::sync::Blocker, Consumer, Cursor, OQueue, OQueueAttachError, Producer, StrongObserver,
    WeakObserver,
};
use crate::{
    prelude::{Arc, Box, Vec},
    sync::{Mutex, WaitQueue, Waker},
    task::Task,
};

/// An OQueue implementation which supports `Send`-only values. It supports an unlimited number of producers and
/// consumers. It does not support observers (weak or strong). It is implemented using a single lock for the entire
/// OQueue.
pub struct LockingQueue<T: UnwindSafe> {
    this: Weak<LockingQueue<T>>,
    inner: Mutex<LockingOQueueInner<T>>,
    buffer_size: usize,
    put_wait_queue: WaitQueue,
    read_wait_queue: WaitQueue,
}

impl<T: UnwindSafe> LockingQueue<T> {
    /// Create a new OQueue with a given size.
    pub fn new(buffer_size: usize) -> Arc<Self> {
        Self::new_with_observers(buffer_size, 0)
    }

    fn new_with_observers(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        Arc::new_cyclic(|this| LockingQueue {
            this: this.clone(),
            buffer_size,
            inner: Mutex::new(LockingOQueueInner {
                buffer: (0..buffer_size).map(|_| None).collect(),
                n_consumers: 0,
                head_index: usize::MAX,
                tail_index: 0,
                free_strong_observer_heads: (0..max_strong_observers).collect(),
                strong_observer_heads: (0..max_strong_observers).map(|_| usize::MAX).collect(),
            }),
            put_wait_queue: Default::default(),
            read_wait_queue: Default::default(),
        })
    }

    fn get_this(&self) -> Result<Arc<LockingQueue<T>>, OQueueAttachError> {
        self.this
            .upgrade()
            .ok_or_else(|| OQueueAttachError::AllocationFailed {
                table_type: type_name::<Self>().to_owned(),
                message: "self was removed from original Arc".to_owned(),
            })
    }
}

/// An OQueue implementation which supports `Send + Clone` values and supports observation. It also supports and unlimited
/// number of producers and consumers. It is implemented using a single lock for the entire OQueue.
pub struct ObservableLockingQueue<T: UnwindSafe> {
    // TODO: This creates a layer of indirection that isn't strictly needed, however removing it is tricky because we
    // have composition not inheritance so the self available in `inner` is "wrong" in that it isn't the value carried
    // around by the `Arc` the user has. This means that the weak-this cannot be correct without having an Arc directly
    // wrapping the OQueue.

    // XXX: Because of the above the outer layer can get collected even if the inner is kept alive by an attachment.
    /// The underlying OQueue used. This can be used to implement the more general observable OQueue because it actually
    /// does support the required features, but only if `T: Clone` and this type is required to guarantee that during
    /// attachment and handle construction.
    inner: Arc<LockingQueue<T>>,
    this: Weak<ObservableLockingQueue<T>>,
}

impl<T: UnwindSafe> ObservableLockingQueue<T> {
    /// Create a new OQueue (with observer support) with the given buffer size and supported strong observers. The cost
    /// of an unused observer is very low, so giving a large value here is reasonable.
    pub fn new(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        let inner = LockingQueue::new_with_observers(buffer_size, max_strong_observers);
        Arc::new_cyclic(|this| ObservableLockingQueue {
            inner,
            this: this.clone(),
        })
    }
}

/// The mutex protected data in the locking OQueue implementations.
struct LockingOQueueInner<T> {
    // TODO:PERFORMANCE: This buffer could use Uninit to save space.
    /// The buffer.
    buffer: Box<[Option<T>]>,

    /// The number of attached consumers.
    n_consumers: usize,
    /// The index from which the next element will be read. Used by consumers.
    head_index: usize,
    /// The index of the next element to write in the buffer. Used by producers.
    tail_index: usize,

    /// The heads used by strong observers.
    strong_observer_heads: Vec<usize>,

    /// A list of strong observer heads that are available to be allocated to an attacher.
    free_strong_observer_heads: Vec<usize>,
}

impl<T> LockingOQueueInner<T> {
    fn mod_len(&self, i: usize) -> usize {
        i % self.buffer.len()
    }

    fn can_produce(&self) -> Option<usize> {
        let head_slot = self.mod_len(self.head_index);

        let next_tail_slot = self.mod_len(self.tail_index + 1);
        if (self.n_consumers > 0 && next_tail_slot == head_slot)
            || (self
                .strong_observer_heads
                .iter()
                .any(|h| *h != usize::MAX && next_tail_slot == self.mod_len(*h)))
        {
            return None;
        }

        Some(self.tail_index)
    }

    fn try_produce(&mut self, v: T) -> Option<T> {
        let Some(tail_index) = self.can_produce() else {
            return Some(v);
        };

        let slot_cell = &mut self.buffer[self.mod_len(tail_index)];
        // This will generally fill something that was None. However, if the this is an observable OQueue then they will
        // be cloned out and left in place. So the cell will still be full.

        // TODO: It might be worth clearing the slot as soon as it is observed since that would avoid holding onto
        // memory.
        *slot_cell = Some(v);

        self.tail_index += 1;

        None
    }

    fn drop_consumer(&mut self) {
        self.n_consumers -= 1;
        if self.n_consumers == 0 {
            self.head_index = usize::MAX;
        }
    }

    fn attach_consumer(&mut self) {
        if self.n_consumers == 0 {
            self.head_index = self.tail_index;
        }
        self.n_consumers += 1;
    }

    fn try_take_for_head(&mut self, head_index: usize) -> Option<&mut Option<T>> {
        if self.mod_len(head_index) == self.mod_len(self.tail_index) {
            debug_assert_eq!(head_index, self.tail_index);
            return None;
        }

        let head_slot = self.mod_len(head_index);
        let slot_cell = &mut self.buffer[head_slot];

        Some(slot_cell)
    }

    fn try_consume(&mut self) -> Option<T> {
        let res = self.try_take_for_head(self.head_index)?;
        let res = res
            .take()
            .expect("empty cell in buffer which should be filled based on indexes");
        self.head_index += 1;
        Some(res)
    }

    fn can_consume(&self) -> bool {
        self.head_index != self.tail_index
    }

    fn try_consume_clone(&mut self) -> Option<T>
    where
        T: Clone,
    {
        let res = self.try_take_for_head(self.head_index)?;
        let res = res
            .clone()
            .expect("empty cell in buffer which should be filled based on indexes");
        self.head_index += 1;
        Some(res)
    }

    fn try_strong_observe(&mut self, observer_index: usize) -> Option<T>
    where
        T: Clone,
    {
        let res = self.try_take_for_head(self.strong_observer_heads[observer_index])?;
        let res = res
            .clone()
            .expect("empty cell in buffer which should be filled based on indexes");
        self.strong_observer_heads[observer_index] += 1;
        Some(res)
    }

    fn can_strong_observe(&self, observer_index: usize) -> bool {
        self.strong_observer_heads[observer_index] != self.tail_index
    }

    fn try_weak_observe(&mut self, index: &Cursor) -> Option<T>
    where
        T: Clone,
    {
        let index = index.index();
        if index < self.tail_index.saturating_sub(self.buffer.len()) || index > self.tail_index {
            return None;
        }
        self.buffer[self.mod_len(index)].clone()
    }
}

impl<T: Clone + Send + UnwindSafe + 'static> OQueue<T> for ObservableLockingQueue<T> {
    fn attach_producer(&self) -> Result<Box<dyn super::Producer<T>>, super::OQueueAttachError> {
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingProducer {
            oqueue: this.this.clone(),
            _oqueue_ref: self.this.upgrade().unwrap(),
        }))
    }

    fn attach_consumer(&self) -> Result<Box<dyn super::Consumer<T>>, super::OQueueAttachError> {
        let this = self.inner.get_this()?;
        this.inner.lock().attach_consumer();
        Ok(Box::new(CloningLockingConsumer {
            oqueue: this.this.clone(),
            _oqueue_ref: self.this.upgrade().unwrap(),
        }))
    }

    fn attach_strong_observer(
        &self,
    ) -> Result<Box<dyn super::StrongObserver<T>>, super::OQueueAttachError> {
        let index = {
            let mut inner = self.inner.inner.lock();
            let index = inner.free_strong_observer_heads.pop().ok_or_else(|| {
                OQueueAttachError::AllocationFailed {
                    table_type: type_name::<Self>().to_owned(),
                    message: format!(
                        "only {} strong observers supported",
                        inner.strong_observer_heads.len()
                    ),
                }
            })?;
            // Start the observer at the current position of the producer.
            inner.strong_observer_heads[index] = inner.tail_index;
            index
        };
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingStrongObserver {
            oqueue: this.this.clone(),
            index,
            _oqueue_ref: self.this.upgrade().unwrap(),
        }))
    }

    fn attach_weak_observer(
        &self,
    ) -> Result<Box<dyn super::WeakObserver<T>>, super::OQueueAttachError> {
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingWeakObserver {
            oqueue: this.this.clone(),
            max_observed_tail: Cell::new(0),
            _oqueue_ref: self.this.upgrade().unwrap(),
        }))
    }
}

impl<T: Send + UnwindSafe + 'static> OQueue<T> for LockingQueue<T> {
    fn attach_producer(&self) -> Result<Box<dyn super::Producer<T>>, super::OQueueAttachError> {
        let this = self.get_this()?;
        Ok(Box::new(LockingProducer {
            oqueue: this.this.clone(),
            _oqueue_ref: this,
        }))
    }

    fn attach_consumer(&self) -> Result<Box<dyn super::Consumer<T>>, super::OQueueAttachError> {
        let this = self.get_this()?;
        this.inner.lock().attach_consumer();
        Ok(Box::new(LockingConsumer { oqueue: this }))
    }

    fn attach_strong_observer(
        &self,
    ) -> Result<Box<dyn super::StrongObserver<T>>, super::OQueueAttachError> {
        Err(OQueueAttachError::Unsupported {
            table_type: type_name::<Self>().to_owned(),
        })
    }

    fn attach_weak_observer(
        &self,
    ) -> Result<Box<dyn super::WeakObserver<T>>, super::OQueueAttachError> {
        Err(OQueueAttachError::Unsupported {
            table_type: type_name::<Self>().to_owned(),
        })
    }
}

/// A producer for a locking OQueue. The same is used regardless of observation support.
struct LockingProducer<T: UnwindSafe> {
    oqueue: Weak<LockingQueue<T>>,
    _oqueue_ref: Arc<dyn Any + Send + Sync + RefUnwindSafe>,
}

impl<T: UnwindSafe> LockingProducer<T> {
    fn oqueue(&self) -> &LockingQueue<T> {
        // SAFETY: This is safe when `oqueue` is referenced by `_oqueue_ref`
        unsafe { &*self.oqueue.as_ptr() }
    }
}

impl<T: Send + UnwindSafe> Blocker for LockingProducer<T> {
    fn should_try(&self) -> bool {
        self.oqueue().inner.lock().can_produce().is_some()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue().put_wait_queue.enqueue(waker.clone());
    }
}

impl<T: Send + UnwindSafe + 'static> Producer<T> for LockingProducer<T> {
    fn produce(&self, data: T) {
        let mut d = Some(data);

        loop {
            d = self.try_produce(d.take().expect("Unreachable"));
            if d.is_none() {
                break;
            }
            Task::current().unwrap().block_on(&[self]);
        }
    }

    fn try_produce(&self, data: T) -> Option<T> {
        let res = self.oqueue().inner.lock().try_produce(data);
        // If the value was put into the OQueue, wake up the readers.
        if res.is_none() {
            // We wake up everyone to make sure we get all the observers. If there are multiple consumers, only one will
            // actually succeed.
            self.oqueue().read_wait_queue.wake_all();
        }
        res
    }
}

/// A consumer for a locking OQueue. This is only used for non-observable tables where the value should be *moved* out
/// instead of cloned.
struct LockingConsumer<T: UnwindSafe> {
    oqueue: Arc<LockingQueue<T>>,
}

impl<T: UnwindSafe> Blocker for LockingConsumer<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().can_consume()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: UnwindSafe> Drop for LockingConsumer<T> {
    fn drop(&mut self) {
        self.oqueue.inner.lock().drop_consumer();
    }
}

impl<T: Send + UnwindSafe + 'static> Consumer<T> for LockingConsumer<T> {
    fn consume(&self) -> T {
        self.block_until(|| self.try_consume())
    }

    fn try_consume(&self) -> Option<T> {
        let res = self.oqueue.inner.lock().try_consume();
        // If a value was taken, wake up a consumer.
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_all();
        }
        res
    }
}

/// A consumer for a locking OQueue which does support observers. This clones values as they are taken out of the OQueue,
/// to make sure they are still available for observers.
struct CloningLockingConsumer<T: UnwindSafe> {
    oqueue: Weak<LockingQueue<T>>,
    // Object that manages the lifetime of `oqueue`
    _oqueue_ref: Arc<dyn Any + Send + Sync + RefUnwindSafe>,
}

impl<T: UnwindSafe> CloningLockingConsumer<T> {
    fn oqueue(&self) -> &LockingQueue<T> {
        // This is safe when `oqueue` is referenced by `_oqueue_ref`
        unsafe { &*self.oqueue.as_ptr() }
    }
}

impl<T: UnwindSafe> Drop for CloningLockingConsumer<T> {
    fn drop(&mut self) {
        self.oqueue().inner.lock().drop_consumer();
    }
}

impl<T: Send + Clone + UnwindSafe + 'static> Consumer<T> for CloningLockingConsumer<T> {
    fn consume(&self) -> T {
        self.block_until(|| self.try_consume())
    }

    fn try_consume(&self) -> Option<T> {
        let res = self.oqueue().inner.lock().try_consume_clone();
        // If a value was taken, wake up a consumer.
        if res.is_some() {
            self.oqueue().put_wait_queue.wake_all();
        }
        res
    }
}

impl<T: UnwindSafe> Blocker for CloningLockingConsumer<T> {
    fn should_try(&self) -> bool {
        self.oqueue().inner.lock().can_consume()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue().read_wait_queue.enqueue(waker.clone())
    }
}

/// A strong observer for a locking OQueue. This will clone values and works only with [`CloningLockingConsumer`].
struct LockingStrongObserver<T: UnwindSafe> {
    oqueue: Weak<LockingQueue<T>>,
    index: usize,
    // Object that manages the lifetime of `oqueue`
    _oqueue_ref: Arc<dyn Any + Send + Sync + RefUnwindSafe>,
}

impl<T: UnwindSafe> LockingStrongObserver<T> {
    fn oqueue(&self) -> &LockingQueue<T> {
        // This is safe when `oqueue` is referenced by `_oqueue_ref`
        unsafe { &*self.oqueue.as_ptr() }
    }
}

impl<T: UnwindSafe> Blocker for LockingStrongObserver<T> {
    fn should_try(&self) -> bool {
        self.oqueue().inner.lock().can_strong_observe(self.index)
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue().read_wait_queue.enqueue(waker.clone());
    }
}

impl<T: UnwindSafe> Drop for LockingStrongObserver<T> {
    fn drop(&mut self) {
        // Free the observer head so that it is available again and ignored.
        let mut inner = self.oqueue().inner.lock();
        inner.strong_observer_heads[self.index] = usize::MAX;
        inner.free_strong_observer_heads.push(self.index);
    }
}

impl<T: Clone + Send + UnwindSafe> StrongObserver<T> for LockingStrongObserver<T> {
    fn strong_observe(&self) -> T {
        self.block_until(|| self.try_strong_observe())
    }

    fn try_strong_observe(&self) -> Option<T> {
        let res = self
            .oqueue()
            .inner
            .try_lock()?
            .try_strong_observe(self.index);
        if res.is_some() {
            self.oqueue().put_wait_queue.wake_all();
        }
        res
    }
}

/// A weak observer for a locking OQueue. This only works with [`ObservableLockingOQueue`] since otherwise the values
/// would have been moved out instead of cloned.
struct LockingWeakObserver<T: UnwindSafe> {
    oqueue: Weak<LockingQueue<T>>,
    max_observed_tail: Cell<usize>,
    // Object that manages the lifetime of `oqueue`
    _oqueue_ref: Arc<dyn Any + Send + Sync + RefUnwindSafe>,
}

impl<T: UnwindSafe> LockingWeakObserver<T> {
    fn oqueue(&self) -> &LockingQueue<T> {
        // This is safe when `oqueue` is referenced by `_oqueue_ref`
        unsafe { &*self.oqueue.as_ptr() }
    }
}

impl<T: UnwindSafe> Blocker for LockingWeakObserver<T> {
    fn should_try(&self) -> bool {
        self.oqueue().inner.lock().tail_index > self.max_observed_tail.get()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue().read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Clone + Send + UnwindSafe> WeakObserver<T> for LockingWeakObserver<T> {
    fn weak_observe(&self, cursor: Cursor) -> Option<T> {
        let mut inner = self.oqueue().inner.lock();
        self.max_observed_tail
            .set(inner.tail_index.max(self.max_observed_tail.get()));
        inner.try_weak_observe(&cursor)
    }

    fn recent_cursor(&self) -> Cursor {
        Cursor(self.oqueue().inner.lock().tail_index.saturating_sub(1))
    }

    fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.recent_cursor();
        // Return the most recent - the buffer size or zero if the buffer isn't full yet.
        if i < self.oqueue().buffer_size {
            Cursor(0)
        } else {
            Cursor(i - (self.oqueue().buffer_size - 1))
        }
    }
}

#[cfg(ktest)]
mod test {
    use ostd_macros::ktest;

    use super::*;
    use crate::orpc::oqueue::generic_test::*;

    #[ktest]
    fn test_produce_consume_locking() {
        let oqueue = LockingQueue::new(2);
        test_produce_consume(oqueue);
    }

    #[ktest]
    fn test_send_multi_receive_blocker_locking() {
        let oqueue1 = LockingQueue::new(10);
        let oqueue2 = LockingQueue::new(10);
        test_send_multi_receive_blocker(oqueue1, oqueue2, 50);
    }

    #[ktest]
    fn test_produce_consume_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_consume(oqueue);
    }

    #[ktest]
    fn test_produce_strong_observe_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_strong_observe(oqueue);
    }

    #[ktest]
    fn test_produce_weak_observe_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_weak_observe(oqueue);
    }

    #[ktest]
    fn test_send_receive_blocker_observable_locking() {
        let oqueue = ObservableLockingQueue::new(10, 5);
        test_send_receive_blocker(oqueue, 100, 5);
    }

    #[ktest]
    fn test_send_multi_receive_blocker_observable_locking() {
        let oqueue1 = ObservableLockingQueue::new(10, 5);
        let oqueue2 = ObservableLockingQueue::new(10, 5);
        test_send_multi_receive_blocker(oqueue1, oqueue2, 50);
    }
}
