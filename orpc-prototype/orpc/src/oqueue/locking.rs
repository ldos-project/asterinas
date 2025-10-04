//! A simple implementation of a [`crate::oqueue::OQueue`] using a [`crate::sync::Mutex`]s. This is a baseline
//! implementation that supports all features, but is also slow in many cases.

use crate::{
    oqueue::{Cursor, OQueue, Receiver, Sender, StrongObserver, OQueueAttachError, WeakObserver},
    sync::{
        Mutex,
        blocker::{Blocker, TaskList},
        task::{Task, TaskRef},
    },
};
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{any::type_name, cell::Cell};
use std::panic::UnwindSafe;

/// A oqueue implementation which supports `Send`-only values. It supports an unlimited number of senders and
/// receivers. It does not support observers (weak or strong). It is implemented using a single lock for the entire
/// oqueue.
pub struct LockingQueue<T> {
    this: Weak<LockingQueue<T>>,
    inner: Mutex<LockingTableInner<T>>,
    buffer_size: usize,
    put_wait_queue: TaskList,
    read_wait_queue: TaskList,
}

static_assertions::assert_impl_all!(LockingQueue<usize>: UnwindSafe);

impl<T> LockingQueue<T> {
    /// Create a new oqueue with a given size.
    pub fn new(buffer_size: usize) -> Arc<Self> {
        Self::new_with_observers(buffer_size, 0)
    }

    fn new_with_observers(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        Arc::new_cyclic(|this| LockingQueue {
            this: this.clone(),
            buffer_size,
            inner: Mutex::new(LockingTableInner {
                buffer: (0..buffer_size).map(|_| None).collect(),
                n_receivers: 0,
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

/// A oqueue implementation which supports `Send + Clone` values and supports observation. It also supports and unlimited
/// number of senders and receivers. It is implemented using a single lock for the entire oqueue.
pub struct ObservableLockingQueue<T> {
    // TODO: This creates a layer of indirection that isn't strictly needed, however removing it is tricky because we
    // have composition not inheritance so the self available in `inner` is "wrong" in that it isn't the value carried
    // around by the `Arc` the user has. This means that the weak-this cannot be correct without having an Arc directly
    // wrapping the oqueue.

    // XXX: Because of the above the outer layer can get collected even if the inner is kept alive by an attachment.
    /// The underlying oqueue used. This can be used to implement the more general observable oqueue because it actually
    /// does support the required features, but only if `T: Clone` and this type is required to guarantee that during
    /// attachment and handle construction.
    inner: Arc<LockingQueue<T>>,
}

impl<T> ObservableLockingQueue<T> {
    /// Create a new oqueue (with observer support) with the given buffer size and supported strong observers. The cost
    /// of an unused observer is very low, so giving a large value here is reasonable.
    pub fn new(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        let inner = LockingQueue::new_with_observers(buffer_size, max_strong_observers);
        Arc::new(ObservableLockingQueue { inner })
    }
}

/// The mutex protected data in the locking oqueue implementations.
struct LockingTableInner<T> {
    // TODO: This buffer could use Uninit to save space.
    /// The buffer.
    buffer: Box<[Option<T>]>,

    /// The number of attached receivers.
    n_receivers: usize,
    /// The index from which the next element will be read. Used by receivers.
    head_index: usize,
    /// The index of the next element to write in the buffer. Used by senders.
    tail_index: usize,

    /// The heads used by strong observers.
    strong_observer_heads: Vec<usize>,

    /// A list of strong observer heads that are available to be allocated to an attacher.
    free_strong_observer_heads: Vec<usize>,
}

impl<T> LockingTableInner<T> {
    fn mod_len(&self, i: usize) -> usize {
        i % self.buffer.len()
    }

    fn can_send(&self) -> Option<usize> {
        let head_slot = self.mod_len(self.head_index);

        let next_tail_slot = self.mod_len(self.tail_index + 1);
        if (self.n_receivers > 0 && next_tail_slot == head_slot)
            || (self
                .strong_observer_heads
                .iter()
                .any(|h| *h != usize::MAX && next_tail_slot == self.mod_len(*h)))
        {
            return None;
        }

        Some(self.tail_index)
    }

    fn try_send(&mut self, v: T) -> Option<T> {
        let Some(tail_index) = self.can_send() else {
            return Some(v);
        };

        let slot_cell = &mut self.buffer[self.mod_len(tail_index)];
        // This will generally fill something that was None. However, if the this is an observable oqueue then they will
        // be cloned out and left in place. So the cell will still be full.

        // TODO: It might be worth clearing the slot as soon as it is observed since that would avoid holding onto
        // memory.
        *slot_cell = Some(v);

        self.tail_index += 1;

        None
    }

    fn drop_receiver(&mut self) {
        self.n_receivers -= 1;
        if self.n_receivers == 0 {
            self.head_index = usize::MAX;
        }
    }

    fn attach_receiver(&mut self) {
        if self.n_receivers == 0 {
            self.head_index = self.tail_index;
        }
        self.n_receivers += 1;
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

    fn try_receive(&mut self) -> Option<T> {
        let res = self.try_take_for_head(self.head_index)?;
        let res = res
            .take()
            .expect("empty cell in buffer which should be filled based on indexes");
        self.head_index += 1;
        Some(res)
    }

    fn can_receive(&self) -> bool {
        self.head_index != self.tail_index
    }

    fn try_receive_clone(&mut self) -> Option<T>
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

impl<T: Clone + Send + 'static> OQueue<T> for ObservableLockingQueue<T> {
    fn attach_sender(&self) -> Result<Box<dyn super::Sender<T>>, super::OQueueAttachError> {
        self.inner.attach_sender()
    }

    fn attach_receiver(&self) -> Result<Box<dyn super::Receiver<T>>, super::OQueueAttachError> {
        let this = self.inner.get_this()?;
        this.inner.lock().attach_receiver();
        Ok(Box::new(CloningLockingReceiver { oqueue: this }))
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
            // Start the observer at the current position of the sender.
            inner.strong_observer_heads[index] = inner.tail_index;
            index
        };
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingStrongObserver {
            oqueue: this,
            index,
        }))
    }

    fn attach_weak_observer(
        &self,
    ) -> Result<Box<dyn super::WeakObserver<T>>, super::OQueueAttachError> {
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingWeakObserver {
            oqueue: this,
            max_observed_tail: Cell::new(0),
        }))
    }
}

impl<T: Send + 'static> OQueue<T> for LockingQueue<T> {
    fn attach_sender(&self) -> Result<Box<dyn super::Sender<T>>, super::OQueueAttachError> {
        let this = self.get_this()?;
        Ok(Box::new(LockingSender { oqueue: this }))
    }

    fn attach_receiver(&self) -> Result<Box<dyn super::Receiver<T>>, super::OQueueAttachError> {
        let this = self.get_this()?;
        this.inner.lock().attach_receiver();
        Ok(Box::new(LockingReceiver { oqueue: this }))
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

/// A sender for a locking oqueue. The same is used regardless of observation support.
struct LockingSender<T> {
    oqueue: Arc<LockingQueue<T>>,
}

static_assertions::assert_impl_all!(LockingSender<usize>: UnwindSafe);

impl<T: Send> Blocker for LockingSender<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().can_send().is_some()
    }

    fn prepare_to_wait(&self, task: &TaskRef) {
        self.oqueue.put_wait_queue.add_task(task)
    }

    fn finish_wait(&self, task: &TaskRef) {
        self.oqueue.put_wait_queue.remove_task(task)
    }
}

impl<T: Send> Sender<T> for LockingSender<T> {
    fn send(&self, data: T) {
        let mut d = Some(data);

        loop {
            d = self.try_send(d.take().expect("Unreachable"));
            if d.is_none() {
                break;
            }
            Task::current().block_on(&[self]);
        }
    }

    fn try_send(&self, data: T) -> Option<T> {
        let res = self.oqueue.inner.lock().try_send(data);
        // If the value was put into the oqueue, wake up the readers.
        if res.is_none() {
            // We wake up everyone to make sure we get all the observers. If there are multiple receivers, only one will
            // actually succeed.
            self.oqueue.read_wait_queue.wake_all();
        }
        res
    }
}

/// A receiver for a locking oqueue. This is only used for non-observable tables where the value should be *moved* out
/// instead of cloned.
struct LockingReceiver<T> {
    oqueue: Arc<LockingQueue<T>>,
}

impl<T> Blocker for LockingReceiver<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().can_receive()
    }

    fn prepare_to_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.add_task(task)
    }

    fn finish_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.remove_task(task);
    }
}

impl<T> Drop for LockingReceiver<T> {
    fn drop(&mut self) {
        self.oqueue.inner.lock().drop_receiver();
    }
}

impl<T: Send> Receiver<T> for LockingReceiver<T> {
    fn receive(&self) -> T {
        self.block_until(|| self.try_receive())
    }

    fn try_receive(&self) -> Option<T> {
        let res = self.oqueue.inner.lock().try_receive();
        // If a value was taken, wake up a sender.
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_all();
        }
        res
    }
}

/// A receiver for a locking oqueue which does support observers. This clones values as they are taken out of the oqueue,
/// to make sure they are still available for observers.
struct CloningLockingReceiver<T> {
    oqueue: Arc<LockingQueue<T>>,
}

impl<T> Drop for CloningLockingReceiver<T> {
    fn drop(&mut self) {
        self.oqueue.inner.lock().drop_receiver();
    }
}

impl<T: Send + Clone> Receiver<T> for CloningLockingReceiver<T> {
    fn receive(&self) -> T {
        self.block_until(|| self.try_receive())
    }

    fn try_receive(&self) -> Option<T> {
        let res = self.oqueue.inner.lock().try_receive_clone();
        // If a value was taken, wake up a sender.
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_all();
        }
        res
    }
}

impl<T> Blocker for CloningLockingReceiver<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().can_receive()
    }

    fn prepare_to_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.add_task(task)
    }

    fn finish_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.remove_task(task);
    }
}

/// A strong observer for a locking oqueue. This will clone values and works only with [`CloningLockingReceiver`].
struct LockingStrongObserver<T> {
    oqueue: Arc<LockingQueue<T>>,
    index: usize,
}

impl<T> Blocker for LockingStrongObserver<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().can_strong_observe(self.index)
    }

    fn prepare_to_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.add_task(task)
    }

    fn finish_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.remove_task(task);
    }
}

impl<T> Drop for LockingStrongObserver<T> {
    fn drop(&mut self) {
        // Free the observer head so that it is available again and ignored.
        let mut inner = self.oqueue.inner.lock();
        inner.strong_observer_heads[self.index] = usize::MAX;
        inner.free_strong_observer_heads.push(self.index);
    }
}

impl<T: Clone + Send> StrongObserver<T> for LockingStrongObserver<T> {
    fn strong_observe(&self) -> T {
        self.block_until(|| self.try_strong_observe())
    }

    fn try_strong_observe(&self) -> Option<T> {
        let res = self.oqueue.inner.try_lock()?.try_strong_observe(self.index);
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_all();
        }
        res
    }
}

/// A weak observer for a locking oqueue. This only works with [`ObservableLockingTable`] since otherwise the values
/// would have been moved out instead of cloned.
struct LockingWeakObserver<T> {
    oqueue: Arc<LockingQueue<T>>,
    max_observed_tail: Cell<usize>,
}

impl<T> Blocker for LockingWeakObserver<T> {
    fn should_try(&self) -> bool {
        self.oqueue.inner.lock().tail_index > self.max_observed_tail.get()
    }

    fn prepare_to_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.add_task(task)
    }

    fn finish_wait(&self, task: &crate::sync::task::TaskRef) {
        self.oqueue.read_wait_queue.remove_task(task);
    }
}

impl<T: Clone + Send> WeakObserver<T> for LockingWeakObserver<T> {
    fn weak_observe(&self, cursor: Cursor) -> Option<T> {
        let mut inner = self.oqueue.inner.lock();
        self.max_observed_tail
            .set(inner.tail_index.max(self.max_observed_tail.get()));
        inner.try_weak_observe(&cursor)
    }
    
    fn wait(&self) {
        Task::current().block_on(&[self]);
    }

    fn recent_cursor(&self) -> Cursor {
        Cursor(self.oqueue.inner.lock().tail_index.saturating_sub(1))
    }

    fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.recent_cursor();
        // Return the most recent - the buffer size or zero if the buffer isn't full yet.
        if i < self.oqueue.buffer_size {
            Cursor(0)
        } else {
            Cursor(i - (self.oqueue.buffer_size - 1))
        }
    }
}

#[cfg(test)]
mod test {
    use crate::oqueue::generic_test::*;

    use super::*;

    #[test]
    fn test_produce_consume_locking() {
        let oqueue = LockingQueue::new(2);
        test_produce_consume(oqueue);
    }

    #[test]
    fn test_send_multi_receive_blocker_locking() {
        let oqueue1 = LockingQueue::new(10);
        let oqueue2 = LockingQueue::new(10);
        test_send_multi_receive_blocker(oqueue1, oqueue2, 50);
    }

    #[test]
    fn test_produce_consume_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_consume(oqueue);
    }

    #[test]
    fn test_produce_strong_observe_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_strong_observe(oqueue);
    }

    #[test]
    fn test_produce_weak_observe_observable_locking() {
        let oqueue = ObservableLockingQueue::new(2, 5);
        test_produce_weak_observe(oqueue);
    }

    #[test]
    fn test_send_receive_blocker_observable_locking() {
        let oqueue = ObservableLockingQueue::new(10, 5);
        test_send_receive_blocker(oqueue, 100, 5);
    }

    #[test]
    fn test_send_multi_receive_blocker_observable_locking() {
        let oqueue1 = ObservableLockingQueue::new(10, 5);
        let oqueue2 = ObservableLockingQueue::new(10, 5);
        test_send_multi_receive_blocker(oqueue1, oqueue2, 50);
    }
}
