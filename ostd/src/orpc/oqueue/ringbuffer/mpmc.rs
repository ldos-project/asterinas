// SPDX-License-Identifier: MPL-2.0
//! A MPMC OQueue implementation with support for strong and weak observation with both static and dynamic configurability.
//! Most of the implementation is inspired by https://github.com/rigtorp/MPMCQueue, the major
//! modification is the support for strong/weak observers.

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::type_name,
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    num::NonZero,
    panic::{RefUnwindSafe, UnwindSafe},
    ptr,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use crossbeam_utils::CachePadded;

use super::Element;
use crate::{
    orpc::oqueue::{
        Blocker, Consumer, Cursor, OQueue, OQueueAttachError, Producer, StrongObserver,
        WeakObserver,
    },
    sync::{Mutex, WaitQueue, Waker},
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
        unsafe {
            let data = ptr::read(self.value.data.get()).assume_init();
            data
        }
    }

    unsafe fn get_maybe_uninit(&self) -> MaybeUninit<T> {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe {
            let data = ptr::read(self.value.data.get());
            data
        }
    }
}

/// The sentinel value acts as a lock. When [`MPMCOQueue::head`] is this value or larger, writes to
/// the queue will be blocked. This is used to ensure that StrongObservers can be initialized with a
/// valid index and avoid races where the StrongObserver index is a value that has already been
/// consumed.
const MPMCOQUEUE_HEAD_SENTINEL: usize = 1 << 63;

pub struct MPMCOQueue<T, const STRONG_OBSERVERS: bool = true, const WEAK_OBSERVERS: bool = true> {
    this: Weak<Self>,
    capacity: NonZero<usize>,
    max_strong_observers: usize,
    slots: Box<[Slot<T>]>,
    /// The index at which elements are produced to.
    head: CachePadded<AtomicUsize>,
    /// The index at which elements are consumed from. Note that unlike StrongObservers, each
    /// message is read once across all consumers.
    tail: CachePadded<AtomicUsize>,
    /// Each tail represents the position of a StrongObserver. Tails may be read by multiple
    /// handles, but will be written to by exactly one handle. Unlike a Consumer, each message is
    /// read once by every StrongObserver.
    strong_observer_tails: Box<[CachePadded<AtomicUsize>]>,
    // TODO(aneesh): do strong_observer_tails actually have to be atomic? It's guaranteed that they
    // have only a single writer.
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

// MPMC OQueue implementation based on rigtorp
impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool>
    MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    pub fn new(size: usize, max_strong_observers: usize) -> Arc<Self> {
        assert!(size.is_power_of_two());
        assert!(size > 0);

        Arc::new_cyclic(|this| MPMCOQueue {
            this: this.clone(),
            capacity: NonZero::new(size).unwrap(),
            max_strong_observers,
            slots: (0..(size + 1)).map(|_| Slot::new()).collect(),
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            strong_observer_tails: (0..max_strong_observers)
                .map(|_| CachePadded::new(AtomicUsize::new(usize::MAX)))
                .collect(),
            n_strong_observers: Mutex::new(0),
            put_wait_queue: Default::default(),
            read_wait_queue: Default::default(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity.into()
    }

    /// "turn" is similar to a generation counter.
    /// N.B.: The way rigtorp/MPMCQueue uses the term turn here is misleading. `Slot`s have a turn,
    /// but this is really the generation. The actual "turn" is either (2 * self.turn(index)) or
    /// (2 * self.turn(index) + 1) for producers and consumers respectively. If I could rename it,
    /// it would be "generation", but this ensures consistency with the rigtorp implementation to
    /// make it easier for new readers to pick up.
    fn turn(&self, index: usize) -> usize {
        index / self.capacity
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
                while self.head.load(Ordering::Relaxed) >= MPMCOQUEUE_HEAD_SENTINEL {}
            } else {
                break head;
            }
        }
    }

    ///
    pub fn produce(&self, data: T)
    where
        T: Send,
    {
        // Update the head position so that other producers can also write
        let head = self.fetch_head_for_produce();
        // Get the slot we will be writing to
        let slot = unsafe { &self.slots.get_unchecked(self.idx(head)) };
        // Wait until the generation count matches the turn. Note that we need an even turn because
        // we are writing.
        while self.turn(head) * 2 != slot.turn.load(Ordering::Acquire) {}
        unsafe { slot.store(data) };
        // Mark the slot as writeable
        slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
    }

    fn try_produce(&self, data: T) -> Option<T>
    where
        T: Send,
    {
        // Get the current head position
        let mut head = self.fetch_head_for_produce();

        // We don't need to check for sentinels from this point onwards. If the sentinal value is
        // set, we will just fail below during one of the compare_exchange calls below.
        loop {
            // Get the slot that we'd like to write to
            let slot = unsafe { &self.slots.get_unchecked(self.idx(head)) };
            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another producer has beaten this thread to the slot.
            if (self.turn(head) * 2) == slot.turn.load(Ordering::Acquire) {
                if let Ok(_) =
                    self.head
                        .compare_exchange(head, head + 1, Ordering::SeqCst, Ordering::SeqCst)
                {
                    // We have exclusive write access to the slot, safe to write.
                    unsafe { slot.store(data) };
                    // Mark the slot as writeable
                    slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
                    return None;
                }
            } else {
                // It was not our turn, but we can opportunistically try again.
                let prev_head = head;
                head = self.head.load(Ordering::Acquire);
                // If it is not our turn and the `head` has not moved, the queue must be full.
                // Return the value back to the user.
                if head == prev_head {
                    return Some(data);
                }
            }
        }
    }

    pub fn consume(&self) -> T {
        let tail = self.tail.fetch_add(1, Ordering::Acquire);
        let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
        let mut prev_slot_turn;
        loop {
            prev_slot_turn = slot.turn.load(Ordering::Acquire);
            if (self.turn(tail) * 2 + 1) == prev_slot_turn {
                break;
            }
        }
        let v = unsafe { slot.get() };
        let next_slot_turn = self.turn(tail) * 2 + 2;
        // Check if there's any StrongObservers that have not yet read this slot.
        if !STRONG_OBSERVERS {
            slot.turn.store(next_slot_turn, Ordering::Release);
        } else if self.strong_observer_tails.iter().all(|t| {
            // The slowest tail updates the slot. The check for the other tail values
            // must be >= and not just >, because it's better for two threads to attempt
            // to redundantly update the slot than to have the slot never update.
            t.load(Ordering::Acquire) >= (tail + 1)
        }) {
            // There might be multiple threads updating this slot at the same time. Do a
            // compare_exchange instead of store because we don't want to overwrite a
            // newer value if some other consumer got to it first.
            loop {
                if let Err(slot_turn) = slot.turn.compare_exchange(
                    prev_slot_turn,
                    next_slot_turn,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    if slot_turn >= next_slot_turn {
                        break;
                    }
                    prev_slot_turn = slot_turn;
                } else {
                    break;
                }
            }
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
            let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another consumer has beaten this thread to the slot.
            if (self.turn(tail) * 2 + 1) == slot.turn.load(Ordering::Acquire) {
                if self
                    .tail
                    .compare_exchange(tail, tail + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // We have exclusive consume access to the slot
                    let v = unsafe { self.slots[self.idx(tail)].get() };
                    let mut prev_slot_turn = slot.turn.load(Ordering::Acquire);
                    let next_slot_turn = self.turn(tail) * 2 + 2;

                    // Check if there's any StrongObservers that have not yet read this slot.
                    if !STRONG_OBSERVERS {
                        slot.turn.store(self.turn(tail) * 2 + 2, Ordering::Release);
                    } else if self.strong_observer_tails.iter().all(|t| {
                        // The slowest tail updates the slot. The check for the other tail values
                        // must be >= and not just >, because it's better for two threads to attempt
                        // to redundantly update the slot than to have the slot never update.
                        t.load(Ordering::Acquire) >= (tail + 1)
                    }) {
                        // There might be multiple threads updating this slot at the same time. Do a
                        // compare_exchange instead of store because we don't want to overwrite a
                        // newer value if some other consumer got to it first.
                        loop {
                            if let Err(slot_turn) = slot.turn.compare_exchange(
                                prev_slot_turn,
                                next_slot_turn,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            ) {
                                if slot_turn >= next_slot_turn {
                                    break;
                                }
                                prev_slot_turn = slot_turn;
                            } else {
                                break;
                            }
                        }
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
        let slot = &self.slots[self.idx(tail)];
        if (self.turn(tail) * 2 + 1) != slot.turn.load(Ordering::Acquire) {
            return None;
        }
        self.strong_observer_tails[observer_id].fetch_add(1, Ordering::Acquire);

        // SAFETY: the check above guarantees that this slot contains valid data
        let v = unsafe { slot.get() };

        let mut prev_slot_turn = slot.turn.load(Ordering::Acquire);
        let next_slot_turn = self.turn(tail) * 2 + 2;

        // See try_consume for details about the slot turn update protocol here.
        if self.tail.load(Ordering::Acquire) >= (tail + 1)
            && self
                .strong_observer_tails
                .iter()
                .all(|t| t.load(Ordering::Acquire) >= (tail + 1))
        {
            loop {
                // Do a compare_exchange instead of store because we don't want to overwrite a newer
                // value if some other consumer got to it first.
                if let Err(slot_turn) = slot.turn.compare_exchange(
                    prev_slot_turn,
                    next_slot_turn,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    if slot_turn >= next_slot_turn {
                        break;
                    }
                    prev_slot_turn = slot_turn;
                }
            }
        }

        Some(v)
    }

    fn size(&self) -> usize {
        self.head.load(Ordering::Relaxed) as usize - self.tail.load(Ordering::Relaxed)
    }

    fn empty(&self) -> bool {
        self.size() <= 0
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
        let sturn = self.slots[self.idx(index)].turn.load(Ordering::Relaxed);
        // If the turn of the slot is either (the current turn and readable), or (the next turn and
        // writable (but not yet written)), then attempt to read.
        let expected_turn = 2 * self.turn(index) + 1;
        if sturn != expected_turn && sturn != (expected_turn + 1) {
            return None;
        }

        let v = unsafe { self.slots[self.idx(index)].get_maybe_uninit().assume_init() };
        // Check if the turn changed while reading. This means that the value we read above is
        // not guaranteed to be the value at `index`, so we should throw it away and return failure.
        if self.slots[self.idx(index)].turn.load(Ordering::Acquire) != sturn {
            return None;
        }
        return Some(v);
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
        // let mut d = data;
        // self.oqueue
        //     .put_wait_queue
        //     .wait_until(|| match self.try_produce(d.take().unwrap()) {
        //         Some(returned) => {
        //             d = Some(returned);
        //             None
        //         }
        //         None => Some(()),
        //     });
        // self.oqueue.read_wait_queue.wake_all();
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
        self.oqueue.consume()
        // .read_wait_queue
        // .wait_until(|| self.try_consume())
    }

    fn try_consume(&self) -> Option<T> {
        // SAFETY: SPSCConsumer is Send, but not Sync, so this can only ever be called from a single thread at a time.
        let res = self.oqueue.try_consume();
        if res.is_some() {
            self.oqueue.put_wait_queue.wake_one();
        }
        res
    }

    // fn enqueue_for_take(&self, waker: Arc<crate::sync::Waker>) {
    //     self.oqueue.read_wait_queue.enqueue(waker);
    // }
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
        // TODO(aneesh) can this just be self.head.load(Relaxed) - self.size()?
        // or maybe even min(self.tails.load)?
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
            // By swapping with the sentinel here we lock all producers, preventing the value at
            // `obs_pos` from being written to.
            let obs_pos = self.head.swap(MPMCOQUEUE_HEAD_SENTINEL, Ordering::Acquire);
            // After this store, consumers cannot consume past the index `obs_pos`.
            self.strong_observer_tails[observer_id].store(obs_pos, Ordering::SeqCst);
            // Allow producers to write to this position again
            self.head.swap(obs_pos, Ordering::Release);

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
}

pub struct Rigtorp<T> {
    this: Weak<Self>,
    capacity: NonZero<usize>,
    slots: Box<[Slot<T>]>,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
}

unsafe impl<T> Sync for Rigtorp<T> {}
unsafe impl<T> Send for Rigtorp<T> {}

impl<T> Rigtorp<T> {
    /// new
    pub fn new(size: usize) -> Arc<Self> {
        assert!(size.is_power_of_two());
        assert!(size > 0);

        Arc::new_cyclic(|this| Rigtorp {
            this: this.clone(),
            capacity: NonZero::new(size).unwrap(),
            slots: (0..(size + 1)).map(|_| Slot::new()).collect(),
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity.into()
    }

    /// "turn" is similar to a generation counter.
    /// N.B.: The way rigtorp/MPMCQueue uses the term turn here is misleading. `Slot`s have a turn,
    /// but this is really the generation. The actual "turn" is either (2 * self.turn(index)) or
    /// (2 * self.turn(index) + 1) for producers and consumers respectively. If I could rename it,
    /// it would be "generation", but this ensures consistency with the rigtorp implementation to
    /// make it easier for new readers to pick up.
    fn turn(&self, index: usize) -> usize {
        index / self.capacity
    }

    /// The physical index in the ringbuffer.
    fn idx(&self, index: usize) -> usize {
        index % self.capacity
    }

    /// produce
    pub fn produce(&self, data: T)
    where
        T: Send,
    {
        // Update the head position so that other producers can also write
        let head = self.head.fetch_add(1, Ordering::Acquire);
        // Get the slot we will be writing to
        let slot = unsafe { &self.slots.get_unchecked(self.idx(head)) };
        // Wait until the generation count matches the turn. Note that we need an even turn because
        // we are writing.
        while self.turn(head) * 2 != slot.turn.load(Ordering::Acquire) {}
        unsafe { slot.store(data) };
        // Mark the slot as writeable
        slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
    }

    /// try_produce
    pub fn try_produce(&self, data: T) -> Option<T>
    where
        T: Send,
    {
        // Get the current head position
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            // Get the slot that we'd like to write to
            let slot = unsafe { &self.slots.get_unchecked(self.idx(head)) };
            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another producer has beaten this thread to the slot.
            if (self.turn(head) * 2) == slot.turn.load(Ordering::Acquire) {
                if let Ok(_) =
                    self.head
                        .compare_exchange(head, head + 1, Ordering::SeqCst, Ordering::SeqCst)
                {
                    // We have exclusive write access to the slot, safe to write.
                    unsafe { slot.store(data) };
                    // Mark the slot as writeable
                    slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
                    return None;
                }
            } else {
                // It was not our turn, but we can opportunistically try again.
                let prev_head = head;
                head = self.head.load(Ordering::Acquire);
                // If it is not our turn and the `head` has not moved, the queue must be full.
                // Return the value back to the user.
                if head == prev_head {
                    return Some(data);
                }
            }
        }
    }

    ///
    pub fn consume(&self) -> T {
        let tail = self.tail.fetch_add(1, Ordering::Acquire);
        let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
        while (self.turn(tail) * 2 + 1) != slot.turn.load(Ordering::Acquire) {}
        let v = unsafe { slot.get() };
        slot.turn.store(self.turn(tail) * 2 + 2, Ordering::Release);
        v
    }

    /// try_consume
    pub fn try_consume(&self) -> Option<T> {
        // Get the current tail position
        let mut tail = self.tail.load(Ordering::Acquire);
        loop {
            // Get the slot we intend to read from
            let slot = unsafe { &self.slots.get_unchecked(self.idx(tail)) };
            // If it is our turn, attempt to atomically update the counter while checking to see if
            // another consumer has beaten this thread to the slot.
            if (self.turn(tail) * 2 + 1) == slot.turn.load(Ordering::Acquire) {
                if self
                    .tail
                    .compare_exchange(tail, tail + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // We have exclusive consume access to the slot
                    let v = unsafe { self.slots[self.idx(tail)].get() };
                    let mut prev_slot_turn = slot.turn.load(Ordering::Acquire);
                    let next_slot_turn = self.turn(tail) * 2 + 2;

                    // There might be multiple threads updating this slot at the same time. Do a
                    // compare_exchange instead of store because we don't want to overwrite a
                    // newer value if some other consumer got to it first.
                    loop {
                        if let Err(slot_turn) = slot.turn.compare_exchange(
                            prev_slot_turn,
                            next_slot_turn,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            if slot_turn >= next_slot_turn {
                                break;
                            }
                            prev_slot_turn = slot_turn;
                        }
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

    ///
    pub fn size(&self) -> usize {
        self.head.load(Ordering::Relaxed) - self.tail.load(Ordering::Relaxed)
    }

    ///
    pub fn empty(&self) -> bool {
        self.size() <= 0
    }

    ///
    // pub fn attach_producer(&self) -> Option<Arc<Self>> {
    //     self.this.upgrade()
    // }

    // ///
    // pub fn attach_consumer(&self) -> Option<Arc<Self>> {
    //     self.this.upgrade()
    // }

    pub fn get_this(&self) -> Arc<Self> {
        self.this.upgrade().unwrap()
    }
}

/// The producer handle for [`Rigtorp`].
pub struct RigtorpProducer<T> {
    oqueue: Arc<Rigtorp<T>>,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T: Copy + Send> Blocker for RigtorpProducer<T> {
    fn should_try(&self) -> bool {
        true
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        panic!("!");
    }
}

impl<T: Copy + Send + 'static> Producer<T> for RigtorpProducer<T> {
    fn produce(&self, data: T) {
        self.oqueue.produce(data);
    }

    fn try_produce(&self, data: T) -> Option<T> {
        panic!("!");
    }
}

/// The consumer handle for [`Rigtorp`].
pub struct RigtorpConsumer<T> {
    oqueue: Arc<Rigtorp<T>>,
    // Make this Send, but not Sync
    _phantom: PhantomData<Cell<()>>,
}

impl<T: Copy + Send> Blocker for RigtorpConsumer<T> {
    fn should_try(&self) -> bool {
        true
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        panic!("!");
    }
}

impl<T: Copy + Send + 'static> Consumer<T> for RigtorpConsumer<T> {
    fn consume(&self) -> T {
        self.oqueue.consume()
    }

    fn try_consume(&self) -> Option<T> {
        panic!("!");
    }
}

impl<T: Copy + Send + 'static> OQueue<T> for Rigtorp<T> {
    fn attach_producer(&self) -> Result<Box<dyn Producer<T>>, OQueueAttachError> {
        Ok(Box::new(RigtorpProducer {
            oqueue: self.get_this(),
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_consumer(&self) -> Result<Box<dyn Consumer<T>>, OQueueAttachError> {
        Ok(Box::new(RigtorpConsumer {
            oqueue: self.get_this(),
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<T>>, OQueueAttachError> {
        Err(OQueueAttachError::AllocationFailed {
            table_type: type_name::<Self>().to_owned(),
            message: "no observer".to_owned(),
        })
    }

    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<T>>, OQueueAttachError> {
        Err(OQueueAttachError::AllocationFailed {
            table_type: type_name::<Self>().to_owned(),
            message: "no observer".to_owned(),
        })
    }
}
