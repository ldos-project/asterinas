// SPDX-License-Identifier: MPL-2.0
//! A MPMC OQueue implementation with support for strong and weak observation with both static and dynamic configurability.

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

struct Slot<T> {
    turn: CachePadded<AtomicUsize>,
    value: Element<T>,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Slot {
            turn: CachePadded::new(AtomicUsize::new(0)),
            value: Element::uninit(),
        }
    }

    unsafe fn store(&self, data: T) {
        unsafe { (*self.value.data.get()).write(data) };
    }

    unsafe fn get(&self) -> T {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe {
            let data = ptr::read(self.value.data.get()).assume_init();
            // TODO(aneesh): is the following necessary?
            // (*self.value.data.get()).assume_init_drop();
            data
        }
    }
}

pub struct MPMCOQueue<T, const STRONG_OBSERVERS: bool = true, const WEAK_OBSERVERS: bool = true> {
    this: Weak<Self>,
    capacity: usize,
    max_strong_observers: usize,
    slots: Box<[Slot<T>]>,
    head: CachePadded<AtomicUsize>,
    /// Each tail represents the position of either a Consumer or a Strong Observer
    /// tails may be read by multiple handles, but will be written to by exactly one handle
    tail: CachePadded<AtomicUsize>,
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

// MPMC OQueue implementation based on rigtorp
impl<T, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool>
    MPMCOQueue<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    pub fn new(size: usize, max_strong_observers: usize) -> Arc<Self> {
        assert!(size.is_power_of_two());

        Arc::new_cyclic(|this| MPMCOQueue {
            this: this.clone(),
            capacity: size,
            max_strong_observers,
            slots: (0..size).map(|_| Slot::new()).collect(),
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            strong_observer_tails: (0..max_strong_observers)
                .map(|_| CachePadded::new(AtomicUsize::new(0)))
                .collect(),
            n_strong_observers: Mutex::new(0),
            put_wait_queue: Default::default(),
            read_wait_queue: Default::default(),
        })
    }

    fn turn(&self, value: usize) -> usize {
        value / self.capacity
    }

    fn idx(&self, value: usize) -> usize {
        value % self.capacity
    }

    fn produce(&self, data: T)
    where
        T: Send,
    {
        let head = self.head.fetch_add(1, Ordering::Acquire);
        let slot = &self.slots[self.idx(head)];
        while self.turn(head) * 2 != slot.turn.load(Ordering::Acquire) {}
        unsafe { slot.store(data) };
        slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
    }

    fn try_produce(&self, data: T) -> Option<T>
    where
        T: Send,
    {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            let slot = &self.slots[self.idx(head)];
            if (self.turn(head) * 2) == slot.turn.load(Ordering::Acquire) {
                if let Ok(_) =
                    self.head
                        .compare_exchange(head, head + 1, Ordering::SeqCst, Ordering::SeqCst)
                {
                    unsafe { slot.store(data) };
                    slot.turn.store(self.turn(head) * 2 + 1, Ordering::Release);
                    return None;
                }
            } else {
                let prev_head = head;
                head = self.head.load(Ordering::Acquire);
                if head == prev_head {
                    return Some(data);
                }
            }
        }
    }

    fn try_consume(&self) -> Option<T> {
        let mut tail = self.tail.load(Ordering::Acquire);
        loop {
            let slot = &self.slots[self.idx(tail)];
            if (self.turn(tail) * 2 + 1) == slot.turn.load(Ordering::Acquire) {
                if self
                    .tail
                    .compare_exchange(tail, tail + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let v = unsafe { self.slots[self.idx(tail)].get() };
                    let mut prev_slot_turn = slot.turn.load(Ordering::Acquire);
                    let next_slot_turn = self.turn(tail) * 2 + 2;

                    // The slowest tail updates the slot
                    if self.strong_observer_tails.iter().all(|t| {
                        // This has to be greater than or equal, to avoid a race. See
                        // `try_observe` below.
                        t.load(Ordering::Acquire) >= tail
                    }) {
                        // Do a compare_exchange instead of store because we don't want to overwrite
                        // a newer value if some other consumer got to it first.
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

        // The slowest tail updates the slot. The check for the other tail values must be >= and not
        // just >, because it's better for two threads to attempt to redundantly update the slot
        // than to have the slot never update.
        if self.tail.load(Ordering::Acquire) >= tail
            && self
                .strong_observer_tails
                .iter()
                .all(|t| t.load(Ordering::Acquire) >= tail)
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
        self.head.load(Ordering::Relaxed) - self.strong_observer_tails[0].load(Ordering::Relaxed)
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
        if self.empty() {
            return None;
        }

        let mut index = cursor.index();
        loop {
            let sturn = self.slots[self.idx(index)].turn.load(Ordering::Relaxed);
            if sturn < (2 * self.turn(index)) {
                break;
            }

            let v = unsafe { self.slots[self.idx(index)].get() };
            if self.slots[self.idx(index)].turn.load(Ordering::Acquire) != sturn {
                index += 1;
                continue;
            }
            return Some(v);
        }
        None
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
        !self.oqueue.empty()
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.put_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Producer<T>
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

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCConsumer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        self.oqueue.size() < self.oqueue.capacity
    }

    fn prepare_to_wait(&self, waker: &Arc<Waker>) {
        self.oqueue.read_wait_queue.enqueue(waker.clone())
    }
}

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Consumer<T>
    for MPMCConsumer<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn consume(&self) -> T {
        self.oqueue
            .read_wait_queue
            .wait_until(|| self.try_consume())
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

impl<T: Copy + Send, const STRONG_OBSERVERS: bool, const WEAK_OBSERVERS: bool> Blocker
    for MPMCStrongObserver<T, STRONG_OBSERVERS, WEAK_OBSERVERS>
{
    fn should_try(&self) -> bool {
        self.oqueue.size() < self.oqueue.capacity
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
        if i < self.oqueue.capacity {
            Cursor(0)
        } else {
            Cursor(i - (self.oqueue.capacity - 1))
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
            self.strong_observer_tails[observer_id]
                .store(self.head.load(Ordering::Relaxed), Ordering::Relaxed);
            (*n_observers) += 1;

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
