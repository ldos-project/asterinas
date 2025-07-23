//! A simple implementation of a [`crate::tables::Table`] using a [`crate::sync::Mutex`]s.

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::type_name;

use crate::{
    sync::{Mutex, WaitQueue},
    tables::{Consumer, Cursor, Producer, StrongObserver, Table, TableAttachError, WeakObserver},
};

/// A table implementation which supports `Send`-only values. It supports an unlimited number of producers and
/// consumers. It does not support observers (weak or strong). It is implemented using a single lock for the entire
/// table.
pub struct LockingTable<T> {
    this: Weak<LockingTable<T>>,
    buffer_size: usize,
    inner: Mutex<LockingTableInner<T>>,
    put_wait_queue: WaitQueue,
    read_wait_queue: WaitQueue,
}

impl<T> LockingTable<T> {
    /// Create a new table with a given size.
    pub fn new(buffer_size: usize) -> Arc<Self> {
        Self::new_with_observers(buffer_size, 0)
    }

    fn new_with_observers(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        Arc::new_cyclic(|this| LockingTable {
            this: this.clone(),
            buffer_size,
            inner: Mutex::new(LockingTableInner {
                buffer: (0..buffer_size).map(|_| None).collect(),
                head_index: 0,
                tail_index: 0,
                free_strong_observer_heads: (0..max_strong_observers).collect(),
                strong_observer_heads: (0..max_strong_observers).map(|_| usize::MAX).collect(),
            }),
            put_wait_queue: Default::default(),
            read_wait_queue: Default::default(),
        })
    }

    fn get_this(&self) -> Result<Arc<LockingTable<T>>, TableAttachError> {
        self.this
            .upgrade()
            .ok_or_else(|| TableAttachError::Whatever {
                message: "self was removed from original Arc".to_owned(),
                source: None,
            })
    }
}

/// A table implementation which supports `Send + Clone` values and supports observation. It also supports and unlimited
/// number of producers and consumers. It is implemented using a single lock for the entire table.
pub struct ObservableLockingTable<T> {
    // TODO: This creates a layer of indirection that isn't strictly needed, however removing it is tricky because we
    // have composition not inheritance so the self available in `inner` is "wrong" in that it isn't the value carried
    // around by the `Arc` the user has. This means that the weak-this cannot be correct without having an Arc directly
    // wrapping the table.

    /// The underlying table used. This can be used to implement the more general observable table because it actually
    /// does support the required features, but only if `T: Clone` and this type is required to guarantee that during
    /// attachment and handle construction.
    inner: Arc<LockingTable<T>>,
}

impl<T> ObservableLockingTable<T> {
    /// Create a new table (with observer support) with the given buffer size and supported strong observers. The cost
    /// of an unused observer is very low, so giving a large value here is reasonable.
    pub fn new(buffer_size: usize, max_strong_observers: usize) -> Arc<Self> {
        let inner = LockingTable::new_with_observers(buffer_size, max_strong_observers);
        Arc::new(ObservableLockingTable { inner })
    }
}

/// The mutex protected data in the locking table implementations.
struct LockingTableInner<T> {
    // TODO: This buffer could use Uninit to save space.
    /// The buffer.
    buffer: Box<[Option<T>]>,

    /// The index from which the next element will be read. Used by consumers.
    head_index: usize,
    /// The index of the next element to write in the buffer. Used by producers.
    tail_index: usize,
    /// The heads used by strong observers.
    strong_observer_heads: Box<[usize]>,

    /// A list of strong observer heads that are available to be allocated to an attacher.
    free_strong_observer_heads: Vec<usize>,
}

impl<T> LockingTableInner<T> {
    fn mod_len(&self, i: usize) -> usize {
        i % self.buffer.len()
    }

    fn can_put(&self) -> Option<usize> {
        let head_slot = self.mod_len(self.head_index);

        let next_tail_slot = self.mod_len(self.tail_index + 1);
        if next_tail_slot == head_slot
            || (self
                .strong_observer_heads
                .iter()
                .any(|h| *h != usize::MAX && next_tail_slot == self.mod_len(*h)))
        {
            return None;
        }

        Some(self.tail_index)
    }

    fn try_put(&mut self, v: T) -> Option<T> {
        let Some(tail_index) = self.can_put() else {
            return Some(v);
        };

        let slot_cell = &mut self.buffer[self.mod_len(tail_index)];
        // This will generally fill something that was None. However, if the this is an observable table then they will
        // be cloned out and left in place. So the cell will still be full.

        // TODO: It might be worth clearing the slot as soon as it is observed since that would avoid holding onto
        // memory.
        *slot_cell = Some(v);

        self.tail_index += 1;

        None
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

    fn try_take(&mut self) -> Option<T> {
        let res = self.try_take_for_head(self.head_index)?;
        let res = res
            .take()
            .expect("empty cell in buffer which should be filled based on indexes");
        self.head_index += 1;
        Some(res)
    }

    fn try_take_clone(&mut self) -> Option<T>
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

impl<T: Clone + Send + 'static> Table<T> for ObservableLockingTable<T> {
    fn attach_producer(&self) -> Result<Box<dyn super::Producer<T>>, super::TableAttachError> {
        self.inner.attach_producer()
    }

    fn attach_consumer(&self) -> Result<Box<dyn super::Consumer<T>>, super::TableAttachError> {
        let this = self.inner.get_this()?;
        Ok(Box::new(CloningLockingConsumer { table: this }))
    }

    fn attach_strong_observer(
        &self,
    ) -> Result<Box<dyn super::StrongObserver<T>>, super::TableAttachError> {
        let index = {
            let mut inner = self.inner.inner.lock();
            let index = inner.free_strong_observer_heads.pop().ok_or_else(|| {
                TableAttachError::AllocationFailed {
                    table_type: type_name::<Self>().to_owned(),
                    reason: format!(
                        "only {} strong observers supported",
                        inner.strong_observer_heads.len()
                    ),
                }
            })?;
            // Start the observer at the current position of the consumer.
            inner.strong_observer_heads[index] = inner.head_index;
            index
        };
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingStrongObserver { table: this, index }))
    }

    fn attach_weak_observer(
        &self,
    ) -> Result<Box<dyn super::WeakObserver<T>>, super::TableAttachError> {
        let this = self.inner.get_this()?;
        Ok(Box::new(LockingWeakObserver { table: this }))
    }
}

impl<T: Send + 'static> Table<T> for LockingTable<T> {
    fn attach_producer(&self) -> Result<Box<dyn super::Producer<T>>, super::TableAttachError> {
        let this = self.get_this()?;
        Ok(Box::new(LockingProducer { table: this }))
    }

    fn attach_consumer(&self) -> Result<Box<dyn super::Consumer<T>>, super::TableAttachError> {
        let this = self.get_this()?;
        Ok(Box::new(LockingConsumer { table: this }))
    }

    fn attach_strong_observer(
        &self,
    ) -> Result<Box<dyn super::StrongObserver<T>>, super::TableAttachError> {
        Err(TableAttachError::Unsupported {
            table_type: type_name::<Self>().to_owned(),
        })
    }

    fn attach_weak_observer(
        &self,
    ) -> Result<Box<dyn super::WeakObserver<T>>, super::TableAttachError> {
        Err(TableAttachError::Unsupported {
            table_type: type_name::<Self>().to_owned(),
        })
    }
}

/// A producer for a locking table. The same is used regardless of observation support.
struct LockingProducer<T> {
    table: Arc<LockingTable<T>>,
}

impl<T: Send> Producer<T> for LockingProducer<T> {
    fn put(&self, data: T) {
        let mut d = Some(data);
        self.table
            .put_wait_queue
            .wait_until(|| match self.try_put(d.take().unwrap()) {
                Some(returned) => {
                    d = Some(returned);
                    None
                }
                None => Some(()),
            });
    }

    fn try_put(&self, data: T) -> Option<T> {
        let res = {
            let mut guard = self.table.inner.lock();
            guard.try_put(data)
        };
        // If the value was put into the table, wake up the readers.
        if res.is_none() {
            // We wake up everyone to make sure we get all the observers. If there are multiple consumers, only one will
            // actually succeed.
            self.table.read_wait_queue.wake_all();
        }
        res
    }
}

/// A consumer for a locking table. This is only used for non-observable tables where the value should be *moved* out
/// instead of cloned.
struct LockingConsumer<T> {
    table: Arc<LockingTable<T>>,
}

impl<T: Send> Consumer<T> for LockingConsumer<T> {
    fn take(&self) -> T {
        self.table.read_wait_queue.wait_until(|| self.try_take())
    }

    fn try_take(&self) -> Option<T> {
        let res = self.table.inner.lock().try_take();
        // If a value was taken, wake up a producer.
        if res.is_some() {
            self.table.put_wait_queue.wake_one();
        }
        res
    }
}

/// A consumer for a locking table which does support observers. This clones values as they are taken out of the table,
/// to make sure they are still available for observers.
struct CloningLockingConsumer<T> {
    table: Arc<LockingTable<T>>,
}

impl<T: Send + Clone> Consumer<T> for CloningLockingConsumer<T> {
    fn take(&self) -> T {
        self.table.read_wait_queue.wait_until(|| self.try_take())
    }

    fn try_take(&self) -> Option<T> {
        let res = self.table.inner.lock().try_take_clone();
        // If a value was taken, wake up a producer.
        if res.is_some() {
            self.table.put_wait_queue.wake_one();
        }
        res
    }
}

/// A strong observer for a locking table. This will clone values and works only with [`CloningLockingConsumer`].
struct LockingStrongObserver<T> {
    table: Arc<LockingTable<T>>,
    index: usize,
}

impl<T> Drop for LockingStrongObserver<T> {
    fn drop(&mut self) {
        // Free the observer head so that it is available again and ignored.
        let mut inner = self.table.inner.lock();
        inner.strong_observer_heads[self.index] = usize::MAX;
        inner.free_strong_observer_heads.push(self.index);
    }
}

impl<T: Clone + Send> StrongObserver<T> for LockingStrongObserver<T> {
    fn strong_observe(&self) -> T {
        self
            .table
            .read_wait_queue
            .wait_until(|| self.try_strong_observe())
    }

    fn try_strong_observe(&self) -> Option<T> {
        let res = self.table.inner.lock().try_strong_observe(self.index);
        if res.is_some() {
            self.table.put_wait_queue.wake_one();
        }
        res
    }
}

/// A weak observer for a locking table. This only works with [`ObservableLockingTable`] since otherwise the values
/// would have been moved out instead of cloned.
struct LockingWeakObserver<T> {
    table: Arc<LockingTable<T>>,
}

impl<T: Clone + Send> WeakObserver<T> for LockingWeakObserver<T> {
    fn weak_observe(&self, cursor: Cursor) -> Option<T> {
        self.table.inner.lock().try_weak_observe(&cursor)
    }

    fn recent_cursor(&self) -> Cursor {
        Cursor(self.table.inner.lock().tail_index.saturating_sub(1))
    }

    fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.recent_cursor();
        // Return the most recent - the buffer size or zero if the buffer isn't full yet.
        if i < self.table.buffer_size {
            Cursor(0)
        } else {
            Cursor(i - (self.table.buffer_size - 1))
        }
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{prelude::*, tables::generic_test};
    #[ktest]
    fn test_produce_consume() {
        generic_test::test_produce_consume(LockingTable::<_>::new(2));
    }

    #[ktest]
    fn test_produce_strong_observe() {
        generic_test::test_produce_strong_observe(ObservableLockingTable::<_>::new(2, 1));
    }

    #[ktest]
    fn test_produce_weak_observe() {
        generic_test::test_produce_weak_observe(ObservableLockingTable::<_>::new(2, 0));
    }

    #[ktest]
    fn test_all() {
        generic_test::test_produce_consume(ObservableLockingTable::<_>::new(2, 1));
        generic_test::test_produce_strong_observe(ObservableLockingTable::<_>::new(2, 1));
        generic_test::test_produce_weak_observe(ObservableLockingTable::<_>::new(2, 1));
    }
}

/// Benchmark functions which are invoked from the benchmark kernel
#[expect(missing_docs)]
pub mod benchmark {
    use core::time::Duration;

    use super::*;
    use crate::{benchmark::run_microbenchmark, tables::generic_benchmarks};

    pub fn benchmark_streaming_produce_consume() {
        for size in [2, 4, 64] {
            for n_tables in [1, 4, 16, 64] {
                run_microbenchmark(
                    |iterations| {
                        let mut tables = Vec::new();
                        for _ in 0..n_tables {
                            tables.push(LockingTable::<usize>::new(size));
                        }
                        let mut reporter = generic_benchmarks::benchmark_streaming_produce_consume(
                            iterations, &tables,
                        );
                        reporter.report("name", &"streaming_produce_consume");
                        reporter.report("queue_size", &size);
                        reporter.report("n_tables", &n_tables);
                        reporter
                    },
                    10000,
                    Duration::from_secs(5),
                )
            }
        }
    }

    pub fn benchmark_call_return() {
        let size = 2;
        for n_pairs in [1, 4, 16, 64] {
            run_microbenchmark(
                |iterations| {
                    let mut reporter =
                        generic_benchmarks::benchmark_call_return(iterations, n_pairs, || {
                            LockingTable::new(size)
                        });
                    reporter.report("name", &"call_return");
                    reporter.report("queue_size", &size);
                    reporter
                },
                10000,
                Duration::from_secs(5),
            );
        }
    }
}

// "down payment"
// ...282
//
// ETFs. Muni bonds. ESC mostly.

// Individual retirement.
// ...
