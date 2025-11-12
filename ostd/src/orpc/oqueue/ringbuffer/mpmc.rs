// SPDX-License-Identifier: MPL-2.0
//! A MPMC OQueue implementation with support for strong and weak observation with both static and dynamic configurability.
//! Most of the implementation is inspired by https://github.com/rigtorp/MPMCQueue, the major
//! modification is the support for strong/weak observers.

use alloc::{
    alloc::{alloc, handle_alloc_error},
    borrow::ToOwned,
    boxed::Box,
    sync::{Arc, Weak},
};
use core::{
    any::type_name,
    cell::Cell,
    marker::PhantomData,
    mem::MaybeUninit,
    num::NonZero,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam_utils::CachePadded;

// SPDX-License-Identifier: MPL-2.0
use super::Element;
use crate::{
    orpc::oqueue::{
        Blocker, Consumer, Cursor, OQueue, OQueueAttachError, Producer, StrongObserver,
        WeakObserver,
    },
    sync::{Mutex, WaitQueue, Waker},
    task::Task,
};

/// A single element of the ringbuffer that tracks the last "turn" that updated it.
struct Slot<T> {
    /// The turn for this slot. When it is even, it is writable. If it was previously written to,
    /// that data has been read by all Consumers and StrongObservers.
    /// If it is odd, it is readable. It cannot be written to because some Consumer or
    /// StrongObserver has not yet read the data.
    turn: CachePadded<AtomicUsize>,
    /// The data being stored in this slot
    value: Element<T>,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Slot {
            turn: CachePadded::new(AtomicUsize::new(0)),
            value: Element::uninit(),
        }
    }

    /// Store `data` in this Slot
    unsafe fn store(&self, data: T) {
        unsafe { (*self.value.data.get()).write(data) };
    }

    /// Get the value present in this Slot. This is only safe if [`Slot::store`] was previously
    /// called.
    unsafe fn get(&self) -> T {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe { ptr::read(self.value.data.get()).assume_init() }
    }

    unsafe fn get_maybe_uninit(&self) -> MaybeUninit<T> {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe { ptr::read(self.value.data.get()) }
    }
}

/// The sentinel value acts as a lock. When [`MPMCOQueue::head`] is this value or larger, writes to
/// the queue will be blocked. This is used to ensure that StrongObservers can be initialized with a
/// valid index and avoid races where the StrongObserver index is a value that has already been
/// consumed. We pick the value 1<<63 so that when producers attempt to do a fetch_add, they don't
/// need to first do a load. It is unlikely that we will have 1<<63 concurrent producers (even
/// try_produce will block on the sentinel) in the time it takes to attach a single observer. This
/// does reduce the limit of elements in the queue from usize::MAX to (1 << 63 - 1).
const MPMCOQUEUE_HEAD_SENTINEL: usize = 1 << 63;

/// MPMCOQueue allows conccurrent producers and consumers. For any produced message it is guaranteed
/// that it will be read by exactly one consumer, all strong observers, and zero or more weak
/// observers. A consumed message cannot be viewed by a future strong observer, but could be viewed
/// by a weak observer.
pub struct MPMCOQueue<T, const STRONG_OBSERVERS: bool = true, const WEAK_OBSERVERS: bool = true> {
    this: Weak<Self>,
    capacity: NonZero<usize>,
    max_strong_observers: usize,
    /// A producer "owns" a slot if it successfully incremented head to one index past the slot and
    /// the slot is marked as ready for writing (even turn). A consumer "owns" a slot if it
    /// successfully incremented tail to one index past the slot and the slot is marked as ready for
    /// reading (odd turn).All other slots are "unowned". Note that the turn of a slot will be
    /// updated from reading to writing only when the slowest consumer or strong observer visits it.
    slots: Box<[Slot<T>]>,
    /// The index at which elements are produced to.
    head: CachePadded<AtomicUsize>,
    /// The index at which elements are consumed from. Note that unlike StrongObservers, each
    /// message is read once across all consumers.
    tail: CachePadded<AtomicUsize>,
    n_consumers: Mutex<usize>,
    /// Each tail represents the position of a StrongObserver. Tails may be read by multiple
    /// handles, but will be written to by exactly one handle. Unlike a Consumer, each message is
    /// read once by every StrongObserver.
    strong_observer_tails: Box<[CachePadded<AtomicUsize>]>,
    n_strong_observers: Mutex<usize>,

    /// Wait queue for threads waiting to put into the queue.
    put_wait_queue: WaitQueue,
    /// Wait queue for threads waiting to read data from the queue.
    read_wait_queue: WaitQueue,
}

unsafe impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Sync
    for MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
}
unsafe impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Send
    for MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
}

// Page aligned allocations are needed for large allocations because the buddy allocator in
// ostd is very simple, and misaligned allocations can lead to fragmentation because the allocator
// doesn't coalesce often enough.
fn allocate_page_aligned_array<T>(len: usize) -> Box<[Slot<T>]> {
    let elem_size = core::mem::size_of::<Slot<T>>();
    let total_size = elem_size * len;

    if total_size < 4096 {
        return (0..(len + 1)).map(|_| Slot::new()).collect();
    }

    // Layout with page alignment
    let layout = core::alloc::Layout::from_size_align(total_size, 4096).expect("invalid layout");

    unsafe {
        let ptr = alloc(layout);
        if ptr.is_null() {
            handle_alloc_error(layout);
        }

        // Initialize each element
        let mut current = ptr as *mut Slot<T>;
        for _ in 0..len {
            ptr::write(current, Slot::new());
            current = current.add(1);
        }

        // Turn into a Box<[Slot<T>]> safely
        let slice = core::slice::from_raw_parts_mut(ptr as *mut Slot<T>, len);
        Box::from_raw(slice)
    }
}

// MPMC OQueue implementation based on rigtorp
impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool>
    MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    /// Create a new MPMCOQueue with a maximum size and a maximum number of strong observers.
    pub fn new(capacity: usize, max_strong_observers: usize) -> Arc<Self> {
        assert!(max_strong_observers == 0 || STRONG_OBSERVERS);
        assert!(capacity.is_power_of_two());
        assert!(capacity > 0);
        assert!(capacity < MPMCOQUEUE_HEAD_SENTINEL);

        Arc::new_cyclic(|this| MPMCOQueue {
            this: this.clone(),
            // SAFETY: we check that size > 0 above
            capacity: unsafe { NonZero::new_unchecked(capacity) },
            max_strong_observers,
            slots: allocate_page_aligned_array(capacity),
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(usize::MAX)),
            n_consumers: Mutex::new(0),
            strong_observer_tails: (0..max_strong_observers)
                .map(|_| CachePadded::new(AtomicUsize::new(usize::MAX)))
                .collect(),
            n_strong_observers: Mutex::new(0),
            put_wait_queue: Default::default(),
            read_wait_queue: Default::default(),
        })
    }

    /// Get the capacity of the queue
    pub fn capacity(&self) -> usize {
        self.capacity.into()
    }

    /// "turn" is similar to a generation counter with an extra bit for indicating whether we expect
    /// the slot to be readable or writable.
    fn turn(&self, index: usize, reading: bool) -> usize {
        2 * (index / self.capacity) + if reading { 1 } else { 0 }
    }

    fn next_turn(&self, index: usize, reading: bool) -> usize {
        self.turn(index, reading) + 2
    }

    /// The physical index in the ringbuffer.
    fn idx(&self, index: usize) -> usize {
        index % self.capacity
    }

    /// Get the current head value, spinning if it is MPMCOQUEUE_HEAD_SENTINEL. i.e. this method
    /// will never return MPMCOQUEUE_HEAD_SENTINEL.
    fn fetch_head_for_produce(&self) -> usize {
        loop {
            let head = self.head.fetch_add(1, Ordering::Acquire);
            if head >= MPMCOQUEUE_HEAD_SENTINEL {
                while self.head.load(Ordering::Relaxed) >= MPMCOQUEUE_HEAD_SENTINEL {
                    // TODO(aneesh): Revisit the need for yield - this might only be needed in our
                    // tests that don't have preemptive schedueling.
                    // Task::yield_now();
                }
            } else {
                break head;
            }
        }
    }

    fn get_slot(&self, position: usize) -> &Slot<T> {
        // SAFETY: self.idx(position) ensures that the index for `slots` is in bounds
        unsafe { self.slots.get_unchecked(self.idx(position)) }
    }

    /// Produce an element onto the queue
    pub fn produce(&self, data: T)
    where
        T: Send,
    {
        // Update the head position so that other producers can also write
        let head = self.fetch_head_for_produce();

        // Get the slot we will be writing to
        let slot = &self.get_slot(head);

        // Check if there are any consumers/observers that can see this message. It's safe to check
        // this here because if we've successfully gotten a head value to produce to, it's
        // guaranteed that all active tails are initialized.
        if self.tail.load(Ordering::Acquire) > head
            && self
                .strong_observer_tails
                .iter()
                .all(|t| t.load(Ordering::Acquire) > head)
        {
            // crate::prelude::println!("No conumsers for pos: {}", head);
            // Mark the slot as writable so that the produce isn't blocked
            slot.turn
                .store(self.next_turn(head, false), Ordering::Release);
            return;
        }

        // Wait until the generation count matches the turn. Note that we need an even turn because
        // we are writing.
        let mut counter = 0;
        while self.turn(head, false) != slot.turn.load(Ordering::Acquire) {
            counter += 1;
            if counter % 1024 == 0 {
                counter = 0;
                // TODO(aneesh): Revisit the need for yield - this might only be needed in our tests
                // that don't have preemptive schedueling.
                // Task::yield_now();
            }
            core::hint::spin_loop();
        }
        unsafe { slot.store(data) };
        // Mark the slot as readable
        slot.turn.store(self.turn(head, true), Ordering::Release);
    }

    fn try_produce(&self, data: T) -> Option<T>
    where
        T: Send,
    {
        // We don't need to check for sentinels from this point onwards. If the sentinal value is
        // set, we will just fail below during one of the compare_exchange calls below.
        loop {
            // Get the current head position
            let head = loop {
                let head = self.head.load(Ordering::Acquire);
                if head >= MPMCOQUEUE_HEAD_SENTINEL {
                    while self.head.load(Ordering::Relaxed) >= MPMCOQUEUE_HEAD_SENTINEL {
                        // TODO(aneesh): Revisit the need for yield - this might only be needed in our tests
                        // that don't have preemptive schedueling.
                        // Task::yield_now();
                    }
                } else {
                    break head;
                }
            };
            // Get the slot that we'd like to write to
            let slot = &self.get_slot(head);

            // Check if there are any consumers/observers that can see this message. It's safe to
            // check this here because if we've successfully gotten a head value to produce to, it's
            // guaranteed that all active tails are initialized.
            if self.tail.load(Ordering::Acquire) > head
                && self
                    .strong_observer_tails
                    .iter()
                    .all(|t| t.load(Ordering::Acquire) > head)
            {
                // Mark the slot as writable so that the produce isn't blocked
                slot.turn
                    .store(self.next_turn(head, false), Ordering::Release);
                return None;
            }

            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another producer has beaten this thread to the slot.
            if self.turn(head, false) == slot.turn.load(Ordering::Acquire) {
                // N.B. (aneesh): I'm not entirely sure why SeqCst is used here, but it matches
                // rigtorp/MPMCQueue
                if self
                    .head
                    .compare_exchange(head, head + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // We have exclusive write access to the slot, safe to write.
                    unsafe { slot.store(data) };
                    // Mark the slot as writeable
                    slot.turn.store(self.turn(head, true), Ordering::Release);
                    return None;
                }
            } else {
                // It was not our turn, but we can opportunistically try again.
                let prev_head = head;
                let next_head = self.head.load(Ordering::Acquire);
                // If it is not our turn and the `head` has not moved, the queue must be full.
                // Return the value back to the user.
                if next_head == prev_head {
                    return Some(data);
                }
            }
        }
    }

    fn mark_slot_as_read<const IS_CONSUMER: bool>(
        &self,
        curr_slot_turn: usize,
        next_slot_turn: usize,
        tail: usize,
    ) {
        let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };

        let mut prev_slot_turn = curr_slot_turn;
        // Check if there's any StrongObservers that have not yet read this slot.
        if (IS_CONSUMER || self.tail.load(Ordering::Acquire) >= (tail + 1))
            && (self.strong_observer_tails.is_empty()
                || self.strong_observer_tails.iter().all(|t| {
                    // The slowest tail updates the slot. The check for the other tail values
                    // must be >= and not just >, because it's better for two threads to attempt
                    // to redundantly update the slot than to have the slot never update.
                    t.load(Ordering::Acquire) >= (tail + 1)
                }))
        {
            // There might be multiple threads updating this slot at the same time. Do a
            // compare_exchange instead of store because we don't want to overwrite a
            // newer value if some other consumer got to it first.
            // N.B. (aneesh): I'm not entirely sure why SeqCst is used here, but it matches
            // rigtorp/MPMCQueue
            while let Err(slot_turn) = slot.turn.compare_exchange(
                prev_slot_turn,
                next_slot_turn,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                // This slot was updated by another consumer/observer, so we don't need to
                // update it anymore.
                if slot_turn >= next_slot_turn {
                    break;
                }
                prev_slot_turn = slot_turn;
            }
        }
    }

    /// Consume an element from the queue
    pub fn consume(&self) -> T {
        let mut tail = self.tail.load(Ordering::Acquire);
        let v = loop {
            let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
            let mut counter = 0;
            while self.turn(tail, true) > slot.turn.load(Ordering::Acquire) {
                counter += 1;
                if counter % 1024 == 0 {
                    counter = 0;
                    // TODO(aneesh): Revisit the need for yield - this might only be needed in our tests
                    // that don't have preemptive schedueling.
                    // Task::yield_now();
                }
                core::hint::spin_loop();
            }
            let v = unsafe { slot.get_maybe_uninit() };
            if let Err(new_tail) =
                self.tail
                    .compare_exchange(tail, tail + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                // We failed the compare exchange - this means that some other consumer stole this
                // value, so we can retry instead.
                tail = new_tail;
            } else {
                break unsafe { v.assume_init() };
            }
        };

        let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
        let next_slot_turn = self.next_turn(tail, false);
        // Check if there's any StrongObservers that have not yet read this slot.
        if !STRONG_OBSERVERS {
            slot.turn.store(next_slot_turn, Ordering::Release);
        } else {
            self.mark_slot_as_read::<true>(self.turn(tail, true), next_slot_turn, tail);
        }
        v
    }

    fn try_consume(&self) -> Option<T> {
        // Unlike rigtorp/MPMCQueue, we only expose `try_consume` and not a `consume`. This is
        // because supporting `consume` with sufficient semantics for observation is hard.

        // Get the current tail position
        let mut tail = self.tail.load(Ordering::Acquire);
        loop {
            // Get the slot we intend to read from
            let slot = &self.get_slot(tail);
            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another consumer has beaten this thread to the slot.
            if self.turn(tail, true) == slot.turn.load(Ordering::Acquire) {
                if self
                    .tail
                    .compare_exchange(tail, tail + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // We have exclusive consume access to the slot
                    let v = unsafe { self.get_slot(tail).get() };
                    let prev_slot_turn = slot.turn.load(Ordering::Acquire);
                    let next_slot_turn = self.next_turn(tail, false);

                    // Check if there's any StrongObservers that have not yet read this slot.
                    if !STRONG_OBSERVERS {
                        slot.turn
                            .store(self.next_turn(tail, false), Ordering::Release);
                    } else {
                        self.mark_slot_as_read::<true>(prev_slot_turn, next_slot_turn, tail);
                    }

                    return Some(v);
                }
            } else {
                let prev_tail = tail;
                tail = self.tail.load(Ordering::Acquire);
                if tail == prev_tail {
                    return None;
                }
            }
        }
    }

    fn try_observe(&self, observer_id: usize) -> Option<T> {
        // TODO(aneesh) instead of just reading the index @ this observer which is hard to guarantee
        // on first read (see attach_strong_observer), what if when the index is consumed (turn >
        // id), we reset the pointer to head and retry?
        let tail = self.strong_observer_tails[observer_id].load(Ordering::Acquire);
        let slot = &self.get_slot(tail);
        if self.turn(tail, true) != slot.turn.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the check above guarantees that this slot contains valid data
        let v = unsafe { slot.get() };

        self.strong_observer_tails[observer_id].fetch_add(1, Ordering::Acquire);
        let prev_slot_turn = slot.turn.load(Ordering::Acquire);
        let next_slot_turn = self.next_turn(tail, false);
        self.mark_slot_as_read::<false>(prev_slot_turn, next_slot_turn, tail);

        Some(v)
    }

    fn size(&self) -> usize {
        self.head.load(Ordering::Relaxed) - self.tail.load(Ordering::Relaxed)
    }

    fn empty(&self) -> bool {
        self.size() == 0
    }

    /// Observe a value in the queue based on a cursor. If value isn't available return `None`. This can happen if
    /// either the value is outside the current range covered by the buffer or the value is overwritten while being
    /// observed. This is all best effort and there are no specific guarantees about the availability of values.
    ///
    /// SAFETY: Must only be called from a single thread at a time with a given observer index. E.i., no object calling
    /// this can be Sync, but it may be Send.
    unsafe fn weak_observe(&self, cursor: Cursor) -> Option<T>
    where
        T: Copy + Send,
    {
        let index = cursor.index();

        if self.empty() && self.head.load(Ordering::Relaxed) == 0 {
            return None;
        }

        // Get the current turn of the slot.
        let slot_turn = self.get_slot(index).turn.load(Ordering::Relaxed);
        // If the turn of the slot is either (the current turn and readable), or (the next turn and
        // writable (but not yet written)), then attempt to read.
        let expected_turn = self.turn(index, true);
        if slot_turn != expected_turn && slot_turn != (expected_turn + 1) {
            return None;
        }

        let v = unsafe { self.get_slot(index).get_maybe_uninit() };
        // Check if the turn changed while reading. This means that the value we read above is
        // not guaranteed to be the value at `index`, so we should throw it away and return failure.
        if self.get_slot(index).turn.load(Ordering::Acquire) != slot_turn {
            return None;
        }
        Some(unsafe { v.assume_init() })
    }

    fn get_this(
        &self,
    ) -> Result<Arc<MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>>, OQueueAttachError> {
        self.this
            .upgrade()
            .ok_or_else(|| OQueueAttachError::AllocationFailed {
                message: "self was removed from original Arc".to_owned(),
                table_type: "".to_owned(),
            })
    }

    /// Attach either a consumer or producer to the queue.
    /// SAFETY: This method MUST only be called from a single thread at a time.
    unsafe fn attach_tail(&self, tail: &AtomicUsize) {
        // If the tail is already initialized, no need to initialize it again
        if tail.load(Ordering::Acquire) == usize::MAX {
            // By swapping with the sentinel here we lock all producers, preventing the value at
            // `obs_pos` from being written to.
            let obs_pos = self.head.swap(MPMCOQUEUE_HEAD_SENTINEL, Ordering::Acquire);
            // After this store, consumers/observers cannot consume past the index `obs_pos`. We do
            // a compare exchange in case some other conccurent call to attach_consumer happened
            // first. In that case, we can just continue and use the initialized value.
            let _ = tail.compare_exchange(usize::MAX, obs_pos, Ordering::SeqCst, Ordering::SeqCst);
            // Allow producers to write to this position again
            self.head.swap(obs_pos, Ordering::Release);
        }
    }
}

/// The producer handle for [`MPMCOQueue`].
pub struct MPMCProducer<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> {
    oqueue: Arc<MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>>,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCProducer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        self.oqueue.size() < self.oqueue.capacity.into()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.put_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send + 'static, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Producer<T>
    for MPMCProducer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn produce(&self, data: T) {
        self.oqueue.produce(data);
        self.oqueue.read_wait_queue.wake_all();
    }

    fn try_produce(&self, data: T) -> Option<T> {
        let res = self.oqueue.try_produce(data);
        if res.is_none() {
            self.oqueue.read_wait_queue.wake_all();
        }
        res
    }
}

/// The consumer handle for [`MPMCOQueue`].
pub struct MPMCConsumer<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> {
    oqueue: Arc<MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>>,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Drop
    for MPMCConsumer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn drop(&mut self) {
        let mut n_consumers = self.oqueue.n_consumers.lock();
        *n_consumers -= 1;
        if *n_consumers == 0 {
            // Safe to write here because we hold the lock on n_consumers
            self.oqueue.tail.store(usize::MAX, Ordering::Relaxed);
        }
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCConsumer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        !self.oqueue.empty()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send + 'static, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Consumer<T>
    for MPMCConsumer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn consume(&self) -> T {
        let v = self.oqueue.consume();
        self.oqueue.put_wait_queue.wake_one();
        v
    }

    fn try_consume(&self) -> Option<T> {
        let res = self.oqueue.try_consume();
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_one();
        }
        res
    }
}

/// The strong observer handle for [`MPMCOQueue`].
pub struct MPMCStrongObserver<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> {
    oqueue: Arc<MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>>,
    observer_id: usize,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Drop
    for MPMCStrongObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn drop(&mut self) {
        let mut n_strong_observers = self.oqueue.n_strong_observers.lock();
        self.oqueue.strong_observer_tails[self.observer_id].store(usize::MAX, Ordering::Relaxed);
        *n_strong_observers -= 1;
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCStrongObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        self.oqueue.size() < self.oqueue.capacity.into()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> StrongObserver<T>
    for MPMCStrongObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn strong_observe(&self) -> T {
        self.oqueue
            .read_wait_queue
            .wait_until(|| self.try_strong_observe())
    }

    fn try_strong_observe(&self) -> Option<T> {
        let res = self.oqueue.try_observe(self.observer_id);
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_one();
        }
        res
    }
}

/// The weak-observer handle for [`MPMCOQueue`].
pub struct MPMCWeakObserver<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> {
    oqueue: Arc<MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>>,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCWeakObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        self.oqueue.size() > 0
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> WeakObserver<T>
    for MPMCWeakObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn weak_observe(&self, cursor: Cursor) -> Option<T> {
        // SAFETY: MPMCConsumer is Send, but not Sync, so this can only ever be called from a single thread at a time.
        unsafe { self.oqueue.weak_observe(cursor) }
    }

    fn recent_cursor(&self) -> Cursor {
        // This estimates the most recent element by looking at the head (which is the next slot to write) and subtracting 1.
        Cursor(self.oqueue.head.load(Ordering::Acquire).saturating_sub(1))
    }

    fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.recent_cursor();
        // Return the most recent - the buffer size or zero if the buffer isn't full yet.
        if i < self.oqueue.capacity.into() {
            Cursor(0)
        } else {
            Cursor(i - (usize::from(self.oqueue.capacity) - 1))
        }
    }
}

impl<T: Copy + Send + 'static, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> OQueue<T>
    for MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn attach_producer(&self) -> Result<Box<dyn Producer<T>>, OQueueAttachError> {
        Ok(Box::new(MPMCProducer {
            oqueue: self.get_this()?,
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_consumer(&self) -> Result<Box<dyn Consumer<T>>, OQueueAttachError> {
        let mut n_consumers = self.n_consumers.lock();
        *n_consumers += 1;
        // Prevent any strong observers from being added while the tail is initializing. Otherwise
        // the atomic swap with the head will need some kind of retry.
        let guard = self.n_strong_observers.lock();
        // SAFETY: this is safe because we have the lock above.
        unsafe { self.attach_tail(&self.tail) };

        drop(guard);

        Ok(Box::new(MPMCConsumer {
            oqueue: self.get_this()?,
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<T>>, OQueueAttachError> {
        let mut n_observers = self.n_strong_observers.lock();
        if *n_observers > (self.max_strong_observers + 1) {
            Err(OQueueAttachError::AllocationFailed {
                table_type: type_name::<Self>().to_owned(),
                message: "consumer already attached".to_owned(),
            })
        } else {
            let observer_id = *n_observers;
            (*n_observers) += 1;
            // SAFETY: this is safe because we have the lock above.
            unsafe { self.attach_tail(&self.strong_observer_tails[observer_id]) };

            let oqueue = self.get_this()?;
            Ok(Box::new(MPMCStrongObserver {
                observer_id,
                oqueue,
                _phantom: PhantomData,
            }) as _)
        }
    }

    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<T>>, OQueueAttachError> {
        Ok(Box::new(MPMCWeakObserver {
            oqueue: self.get_this()?,
            _phantom: PhantomData,
        }))
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{orpc::oqueue::generic_test, prelude::*};
    #[ktest]
    fn test_produce_consume() {
        generic_test::test_produce_consume(MPMCOQueue::<_>::new(1, 0));
    }

    #[ktest]
    fn test_produce_strong_observe() {
        generic_test::test_produce_strong_observe(MPMCOQueue::<_>::new(1, 1));
    }

    #[ktest]
    fn test_produce_weak_observe() {
        generic_test::test_produce_weak_observe(MPMCOQueue::<_>::new(2, 1));
    }

    #[ktest]
    fn test_all() {
        generic_test::test_produce_consume(MPMCOQueue::<_>::new(1, 1));
        generic_test::test_produce_strong_observe(MPMCOQueue::<_>::new(1, 1));
        generic_test::test_produce_weak_observe(MPMCOQueue::<_>::new(2, 1));
    }

    #[ktest]
    fn test_send_receive_blocker_observable_mpmc() {
        let oqueue = MPMCOQueue::<_>::new(16, 5);
        generic_test::test_send_receive_blocker(oqueue, 100, 5);
    }

    #[ktest]
    fn test_send_multi_receive_blocker_observable_mpmc() {
        let oqueue1 = MPMCOQueue::<_>::new(16, 5);
        let oqueue2 = MPMCOQueue::<_>::new(16, 5);
        generic_test::test_send_multi_receive_blocker(oqueue1, oqueue2, 50);
    }

    #[ktest]
    fn test_produce_strong_observe_only() {
        generic_test::test_produce_strong_observe_only(MPMCOQueue::<_>::new(1, 1));
    }

    #[ktest]
    fn test_consumer_late_attach() {
        generic_test::test_consumer_late_attach(MPMCOQueue::<_>::new(2, 1));
    }

    #[ktest]
    fn test_consumer_detach() {
        generic_test::test_consumer_detach(MPMCOQueue::<_>::new(2, 1));
    }

    #[ktest]
    fn test_strong_observer_detach() {
        generic_test::test_strong_observer_detach(MPMCOQueue::<_>::new(2, 1));
    }

    #[ktest]
    fn test_strong_observer_late_attach() {
        generic_test::test_strong_observer_late_attach(MPMCOQueue::<_>::new(2, 1));
    }
}
