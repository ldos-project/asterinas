// SPDX-License-Identifier: MPL-2.0
//
#![allow(unsafe_code)]
use alloc::{
    alloc::{alloc, handle_alloc_error},
    borrow::ToOwned,
    boxed::Box,
    sync::{Arc, Weak},
};
use core::{
    any::type_name,
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    num::NonZero,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam_utils::CachePadded;
use ostd::{
    orpc::{
        oqueue::{Consumer, OQueue, OQueueAttachError, Producer, StrongObserver, WeakObserver},
        sync::Blocker,
    },
    sync::Waker,
};

use super::{Benchmark, BenchmarkHarness, time, *};
use crate::{kcmdline::get_kernel_cmd_line, thread::kernel_thread::ThreadOptions};

/// A single element (slot for storing a value) in a ring buffer.
#[derive(Debug)]
struct Element<T> {
    /// The data stored in this element of the ring buffer. This is value is initialized if either the valid bit in
    /// `weak_reader_states` is set or this element is between the head (read) and tail (write) indexes of the ring
    /// buffer. This assumes correct synchronization using the various atomic values used by the ring buffer.
    data: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Element<T> {
    fn uninit() -> Element<T> {
        Element {
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

impl<T: Default> Default for Element<T> {
    fn default() -> Self {
        Self::uninit()
    }
}

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

    #[expect(unused)]
    unsafe fn get_maybe_uninit(&self) -> MaybeUninit<T> {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe { ptr::read(self.value.data.get()) }
    }
}

fn allocate_page_aligned_array<T>(len: usize) -> Box<[Slot<T>]> {
    let elem_size = core::mem::size_of::<Slot<T>>();
    let total_size = elem_size * len;

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
    pub fn new(size: usize) -> Arc<Self> {
        assert!(size.is_power_of_two());
        assert!(size > 0);

        Arc::new_cyclic(|this| Rigtorp {
            this: this.clone(),
            capacity: NonZero::new(size).unwrap(),
            slots: allocate_page_aligned_array(size),
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

    #[expect(unused)]
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
                if self
                    .head
                    .compare_exchange(head, head + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
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
        while (self.turn(tail) * 2 + 1) != slot.turn.load(Ordering::Acquire) {}
        let v = unsafe { slot.get() };
        slot.turn.store(self.turn(tail) * 2 + 2, Ordering::Release);
        v
    }

    #[expect(unused)]
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

    pub fn size(&self) -> usize {
        self.head.load(Ordering::Relaxed) - self.tail.load(Ordering::Relaxed)
    }

    #[expect(unused)]
    pub fn empty(&self) -> bool {
        self.size() == 0
    }

    /// Get a reference to this OQueue
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

    fn prepare_to_wait(&self, _waker: &Arc<Waker>) {
        panic!("!");
    }
}

impl<T: Copy + Send + 'static> Producer<T> for RigtorpProducer<T> {
    fn produce(&self, data: T) {
        self.oqueue.produce(data);
    }

    fn try_produce(&self, _data: T) -> Option<T> {
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

    fn prepare_to_wait(&self, _waker: &Arc<Waker>) {
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
    fn attach_producer(&self) -> core::result::Result<Box<dyn Producer<T>>, OQueueAttachError> {
        Ok(Box::new(RigtorpProducer {
            oqueue: self.get_this(),
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_consumer(&self) -> core::result::Result<Box<dyn Consumer<T>>, OQueueAttachError> {
        Ok(Box::new(RigtorpConsumer {
            oqueue: self.get_this(),
            _phantom: PhantomData,
        }) as _)
    }

    fn attach_strong_observer(
        &self,
    ) -> core::result::Result<Box<dyn StrongObserver<T>>, OQueueAttachError> {
        Err(OQueueAttachError::AllocationFailed {
            table_type: type_name::<Self>().to_owned(),
            message: "no observer".to_owned(),
        })
    }

    fn attach_weak_observer(
        &self,
    ) -> core::result::Result<Box<dyn WeakObserver<T>>, OQueueAttachError> {
        Err(OQueueAttachError::AllocationFailed {
            table_type: type_name::<Self>().to_owned(),
            message: "no observer".to_owned(),
        })
    }
}

pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;

struct OQueueBenchmarkInput {
    pub n_threads: usize,
    pub n_messages: usize,
    pub q_type: String,
}

fn produce_bench(
    input: &OQueueBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    println!("Starting producers");
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));
    // Start all producers
    for tid in 0..input.n_threads {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                for _ in 0..N_MESSAGES_PER_THREAD {
                    producer.produce(0);
                }
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[producer-{}-{:?}] sent msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
}

fn consume_bench(
    input: &OQueueBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    let produced_completed_wq = Arc::new(ostd::sync::WaitQueue::new());
    let produce_completed = Arc::new(AtomicUsize::new(0));

    // Initialize consumer so that the producer knows to retain values
    let consumer = q.attach_consumer().unwrap();
    println!("Populating queue");
    for tid in 0..input.n_threads {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let produce_completed = produce_completed.clone();
            let produced_completed_wq = produced_completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                for _ in 0..N_MESSAGES_PER_THREAD {
                    producer.produce(0);
                }
                produce_completed.fetch_add(1, Ordering::Relaxed);
                produced_completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
    produced_completed_wq
        .wait_until(|| (completed.load(Ordering::Relaxed) == input.n_threads).then_some(()));

    let barrier = Arc::new(AtomicUsize::new(input.n_threads));
    // Start all consumers
    for tid in 0..input.n_threads {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let consumer = q.attach_consumer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                let now = time::clocks::RealTimeClock::get().read_time();
                for _ in 0..N_MESSAGES_PER_THREAD {
                    let _ = consumer.consume();
                }
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[consumer-{}-{:?}] recv msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
    drop(consumer);
}

fn mixed_bench(
    input: &OQueueBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    producer.produce(0);
                }
                crate::prelude::println!("finished producer {}", tid);
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let consumer = q.attach_consumer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
                crate::prelude::println!("finished consumer {}", tid);
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
}

fn weak_obs_bench(
    input: &OQueueBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    producer.produce(0);
                }
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 1).unwrap());
    ThreadOptions::new({
        let completed = completed.clone();
        let consumer = q.attach_consumer().unwrap();
        let barrier = barrier.clone();
        move || {
            barrier.fetch_sub(1, Ordering::Acquire);
            while barrier.load(Ordering::Relaxed) > 0 {}
            for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                let _ = consumer.consume();
            }
            completed.fetch_add(1, Ordering::Relaxed);
        }
    })
    .cpu_affinity(cpu_set)
    .spawn();

    if input.q_type == "mpmc_oq" || input.q_type == "locking" {
        // Start all weak observers
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 2).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let weak_observer = q.attach_weak_observer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                        cnt += weak_observer.weak_observe_recent(1).len();
                    }
                    crate::prelude::println!("weak observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    } else {
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 2).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let weak_observer = q.attach_consumer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                        cnt += weak_observer.consume();
                    }
                    crate::prelude::println!("weak observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    }
}

fn strong_obs_bench(
    input: &OQueueBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                crate::prelude::println!("producer start");
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    producer.produce(0);
                }
                crate::prelude::println!("producer stop");
                completed.fetch_add(1, Ordering::Relaxed);
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 1).unwrap());
    ThreadOptions::new({
        let completed = completed.clone();
        let consumer = q.attach_consumer().unwrap();
        let barrier = barrier.clone();
        move || {
            barrier.fetch_sub(1, Ordering::Acquire);
            // ostd::task::Task::yield_now();
            while barrier.load(Ordering::Relaxed) > 0 {}
            crate::prelude::println!("consumer start");
            for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                let _ = consumer.consume();
            }
            crate::prelude::println!("consumer stop");
            completed.fetch_add(1, Ordering::Relaxed);
        }
    })
    .cpu_affinity(cpu_set)
    .spawn();

    if input.q_type == "mpmc_oq" || input.q_type == "locking" {
        // Start all consumers
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 2 + tid).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let strong_observer = q.attach_strong_observer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    crate::prelude::println!("observer start");
                    let mut cnt = 0;
                    for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                        strong_observer.strong_observe();
                        cnt += 1;
                    }
                    crate::prelude::println!("strong observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    } else {
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 2 + tid).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let strong_observer = q.attach_consumer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                        cnt += strong_observer.consume();
                    }
                    crate::prelude::println!("strong observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    }
}

type OQueueBenchFn =
    &'static dyn Fn(&OQueueBenchmarkInput, &Arc<dyn OQueue<u64>>, &Arc<AtomicUsize>);

struct OQueueBenchmark {
    fn_: OQueueBenchFn,
    name: String,
    input: Option<OQueueBenchmarkInput>,
}

impl OQueueBenchmark {
    fn new(fn_: OQueueBenchFn, name: &str) -> Box<Self> {
        let name = name.to_string();
        Box::new(Self {
            fn_,
            name,
            input: None,
        })
    }

    fn get_oq(&self) -> Arc<dyn OQueue<u64>> {
        let input = self.input.as_ref().unwrap();
        let q_type = &input.q_type;
        let n_messages = input.n_messages;

        if q_type == "mpmc_oq" {
            let q = ostd::orpc::oqueue::ringbuffer::mpmc::MPMCOQueue::<u64>::new(2 << 20, 16);
            assert!(q.capacity() >= n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else if q_type == "locking" {
            let q = ostd::orpc::oqueue::locking::ObservableLockingQueue::<u64>::new(2 << 20, 16);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else {
            let q = Rigtorp::<u64>::new(2 << 20);
            assert!(q.capacity() >= n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        }
    }
}

impl Benchmark for OQueueBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        let karg = get_kernel_cmd_line().unwrap();
        self.input = Some(OQueueBenchmarkInput {
            n_threads,
            n_messages: N_MESSAGES_PER_THREAD * n_threads,
            q_type: karg
                .get_module_arg_by_name::<String>("bench", "q_type")
                .unwrap(),
        });
    }

    fn run(&self, completed: Arc<AtomicUsize>) {
        let q: Arc<dyn OQueue<u64>> = self.get_oq();
        let input = self.input.as_ref().unwrap();
        (self.fn_)(input, &q, &completed);
    }

    fn name(&self) -> &str {
        &self.name
    }
}

pub fn register_benchmarks(bc: &mut BenchmarkHarness) {
    bc.register_benchmark(OQueueBenchmark::new(
        &produce_bench,
        "oqueue::produce_bench",
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        &consume_bench,
        "oqueue::consume_bench",
    ));
    bc.register_benchmark(OQueueBenchmark::new(&mixed_bench, "oqueue::mixed_bench"));
    bc.register_benchmark(OQueueBenchmark::new(
        &weak_obs_bench,
        "oqueue::weak_obs_bench",
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        &strong_obs_bench,
        "oqueue::strong_obs_bench",
    ));
}
