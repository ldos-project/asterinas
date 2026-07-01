// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]
#![allow(unsafe_code)]
use alloc::{
    alloc::{alloc, handle_alloc_error},
    borrow::ToOwned,
    boxed::Box,
    string::ToString as _,
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
    info,
    orpc::{
        oqueue::{
            ConsumableOQueue, ConsumableOQueueRef, OQueue as OtherOQueue, OQueueRef,
            ObservationQuery,
        },
        sync::Blocker,
    },
    sync::{Waker, WakerKey},
};

use super::{Benchmark, BenchmarkHarness, time, *};
use crate::{
    benchmarks::legacy_oqueue::{
        Consumer, Cursor, OQueue, OQueueAttachError, Producer, StrongObserver, WeakObserver,
        ringbuffer::MPMCOQueue,
    },
    kcmdline::get_kernel_cmd_line,
    thread::kernel_thread::ThreadOptions,
};

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

    fn enqueue(&self, _waker: &Arc<Waker>) -> WakerKey {
        panic!("!");
    }

    fn remove(&self, _key: ostd::sync::WakerKey) {
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

    fn enqueue(&self, _waker: &Arc<Waker>) -> WakerKey {
        panic!("!");
    }

    fn remove(&self, _key: ostd::sync::WakerKey) {
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

struct OQueueLegacyBenchmarkInput {
    pub n_threads: usize,
    pub n_messages: usize,
    pub q_type: String,
}

fn spawn_on_cpu<F>(cpu_idx: usize, f: F)
where
    F: FnOnce() + Send + 'static,
{
    let mut cpu_set = ostd::cpu::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(cpu_idx).unwrap());
    ThreadOptions::new(f).cpu_affinity(cpu_set).spawn();
}

fn spawn_bench_thread<F>(
    cpu_idx: usize,
    barrier: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
    label: &'static str,
    timed: bool,
    f: F,
) where
    F: FnOnce() + Send + 'static,
{
    spawn_on_cpu(cpu_idx, move || {
        barrier.fetch_sub(1, Ordering::Acquire);
        while barrier.load(Ordering::Relaxed) > 0 {
            core::hint::spin_loop();
        }

        if timed {
            let now = time::clocks::RealTimeClock::get().read_time();
            f();
            let end = time::clocks::RealTimeClock::get().read_time();
            println!(
                "[{}-{:?}] done in {:?}",
                label,
                ostd::cpu::CpuId::current_racy(),
                end - now
            );
        } else {
            f();
        }

        completed.fetch_add(1, Ordering::Relaxed);
    });
}

struct BenchThreadConfig {
    pub n_threads: usize,
    pub n_messages: usize,
    pub barrier: Arc<AtomicUsize>,
    pub completed: Arc<AtomicUsize>,
    pub label: &'static str,
    pub cpu_offset: usize,
}

fn run_bench_threads<Setup, Work>(
    config: BenchThreadConfig,
    mut setup: Setup,
    on_complete: impl Fn() + Send + Sync + 'static,
) where
    Setup: FnMut() -> Work,
    Work: FnMut() + Send + 'static,
{
    let on_complete = Arc::new(on_complete);
    for tid in 0..config.n_threads {
        let mut work = setup();
        let barrier = config.barrier.clone();
        let completed = config.completed.clone();
        let on_complete = on_complete.clone();
        let n_messages = config.n_messages;
        spawn_bench_thread(
            tid + config.cpu_offset,
            barrier,
            completed,
            config.label,
            true,
            move || {
                for _ in 0..n_messages {
                    work();
                }
                on_complete();
            },
        );
    }
}

fn produce_bench_legacy(
    input: &OQueueLegacyBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));
    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier,
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_producer().unwrap();
            move || producer.produce(0)
        },
        || {},
    );
}

fn produce_bench<Q: OtherOQueue<u64>>(
    input: &OQueueBenchmarkInput,
    q: &Arc<Q>,
    completed: &Arc<AtomicUsize>,
) {
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));
    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier,
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_ref_producer().unwrap();
            move || producer.produce_ref(&0u64)
        },
        || {},
    );
}

fn consume_bench_legacy(
    input: &OQueueLegacyBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    // Attach consumer so the queue knows to retain values
    let _consumer = q.attach_consumer().unwrap();

    let produced_completed_wq = Arc::new(ostd::sync::WaitQueue::new());
    let produce_completed = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier,
            completed: Arc::new(AtomicUsize::new(0)),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        {
            let produce_completed = produce_completed.clone();
            let produced_completed_wq = produced_completed_wq.clone();
            move || {
                produce_completed.fetch_add(1, Ordering::Relaxed);
                produced_completed_wq.wake_all();
            }
        },
    );

    // Wait for queue to be full
    produced_completed_wq.wait_until(|| {
        (produce_completed.load(Ordering::Relaxed) == input.n_threads).then_some(())
    });

    let consume_barrier = Arc::new(AtomicUsize::new(input.n_threads));
    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier: consume_barrier,
            completed: completed.clone(),
            label: "consumer",
            cpu_offset: 1,
        },
        || {
            let consumer = q.attach_consumer().unwrap();
            move || {
                let _ = consumer.consume();
            }
        },
        || {},
    );
}

fn consume_bench<Q: ConsumableOQueue<u64>>(
    input: &OQueueBenchmarkInput,
    q: &Arc<Q>,
    completed: &Arc<AtomicUsize>,
) {
    // Attach consumers first so the queue knows to retain messages
    let _consumers: Vec<_> = (0..input.n_threads)
        .map(|_| q.attach_consumer().unwrap())
        .collect();

    let produced_completed_wq = Arc::new(ostd::sync::WaitQueue::new());
    let produce_completed = Arc::new(AtomicUsize::new(0));
    let produce_barrier = Arc::new(AtomicUsize::new(input.n_threads));

    println!("Populating queue");
    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier: produce_barrier,
            completed: Arc::new(AtomicUsize::new(0)),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_value_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        {
            let produce_completed = produce_completed.clone();
            let produced_completed_wq = produced_completed_wq.clone();
            move || {
                produce_completed.fetch_add(1, Ordering::Relaxed);
                produced_completed_wq.wake_all();
            }
        },
    );

    // Wait for queue to be full
    produced_completed_wq.wait_until(|| {
        (produce_completed.load(Ordering::Relaxed) == input.n_threads).then_some(())
    });

    let consume_barrier = Arc::new(AtomicUsize::new(input.n_threads));
    run_bench_threads(
        BenchThreadConfig {
            n_threads: input.n_threads,
            n_messages: N_MESSAGES_PER_THREAD,
            barrier: consume_barrier,
            completed: completed.clone(),
            label: "consumer",
            cpu_offset: 1,
        },
        || {
            let consumer = q.attach_consumer().unwrap();
            move || {
                let _ = consumer.consume();
            }
        },
        || {},
    );
}

fn mixed_bench_legacy(
    input: &OQueueLegacyBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    // number of threads MUST be even because an equal number of producers and consumers are created
    assert!(
        input.n_threads.is_multiple_of(2),
        "mixed_bench_legacy: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;

    let barrier = Arc::new(AtomicUsize::new(input.n_threads));
    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "consumer",
            cpu_offset: n_threads_per_type + 1,
        },
        || {
            let consumer = q.attach_consumer().unwrap();
            move || {
                let _ = consumer.consume();
            }
        },
        || {},
    );
}

fn mixed_bench<Q: ConsumableOQueue<u64>>(
    input: &OQueueBenchmarkInput,
    q: &Arc<Q>,
    completed: &Arc<AtomicUsize>,
) {
    // number of threads MUST be even because an equal number of producers and consumers are created
    assert!(
        input.n_threads.is_multiple_of(2),
        "mixed_bench: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_value_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "consumer",
            cpu_offset: n_threads_per_type + 1,
        },
        || {
            let consumer = q.attach_consumer().unwrap();
            move || {
                let _ = consumer.consume();
            }
        },
        || {},
    );
}

fn weak_observer_bench_legacy(
    input: &OQueueLegacyBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    assert!(
        input.n_threads.is_multiple_of(2),
        "weak_observer_bench_legacy: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    spawn_bench_thread(
        n_threads_per_type + 1,
        barrier.clone(),
        completed.clone(),
        "consumer",
        false,
        {
            let consumer = q.attach_consumer().unwrap();
            move || {
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
            }
        },
    );

    if input.q_type == "mpmc_oq" || input.q_type == "locking" {
        run_bench_threads(
            BenchThreadConfig {
                n_threads: n_threads_per_type.wrapping_sub(1),
                n_messages: 2 * N_MESSAGES_PER_THREAD,
                barrier: barrier.clone(),
                completed: completed.clone(),
                label: "weak_observer",
                cpu_offset: n_threads_per_type + 2,
            },
            || {
                let weak_observer = q.attach_weak_observer().unwrap();
                move || {
                    weak_observer.weak_observe_recent(1);
                }
            },
            || {},
        );
    } else {
        run_bench_threads(
            BenchThreadConfig {
                n_threads: n_threads_per_type.wrapping_sub(1),
                n_messages: 2 * N_MESSAGES_PER_THREAD,
                barrier: barrier.clone(),
                completed: completed.clone(),
                label: "consumer",
                cpu_offset: n_threads_per_type + 2,
            },
            || {
                let consumer = q.attach_consumer().unwrap();
                move || {
                    let _ = consumer.consume();
                }
            },
            || {},
        );
    }
}

fn weak_observer_bench<Q: ConsumableOQueue<u64>>(
    input: &OQueueBenchmarkInput,
    q: &Arc<Q>,
    completed: &Arc<AtomicUsize>,
) {
    assert!(
        input.n_threads.is_multiple_of(2),
        "weak_observer_bench: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_value_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    spawn_bench_thread(
        n_threads_per_type + 1,
        barrier.clone(),
        completed.clone(),
        "consumer",
        false,
        {
            let consumer = q.attach_consumer().unwrap();
            move || {
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
            }
        },
    );

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type.wrapping_sub(1),
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "weak_observer",
            cpu_offset: n_threads_per_type + 2,
        },
        || {
            let weak_observer = q
                .attach_weak_observer(1, ObservationQuery::new(|v: &u64| *v))
                .unwrap();
            move || {
                let _ = weak_observer.weak_observe_recent(1);
            }
        },
        || {},
    );
}

fn strong_observer_bench_legacy(
    input: &OQueueLegacyBenchmarkInput,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
) {
    assert!(
        input.n_threads.is_multiple_of(2),
        "strong_observer_bench_legacy: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    spawn_bench_thread(
        n_threads_per_type + 1,
        barrier.clone(),
        completed.clone(),
        "consumer",
        false,
        {
            let consumer = q.attach_consumer().unwrap();
            move || {
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
            }
        },
    );

    if input.q_type == "mpmc_oq" || input.q_type == "locking" {
        info!("strong observer bench starting");
        run_bench_threads(
            BenchThreadConfig {
                n_threads: n_threads_per_type.wrapping_sub(1),
                n_messages: 2 * N_MESSAGES_PER_THREAD,
                barrier: barrier.clone(),
                completed: completed.clone(),
                label: "strong_observer",
                cpu_offset: n_threads_per_type + 2,
            },
            || {
                let strong_observer = q.attach_strong_observer().unwrap();
                move || {
                    let _ = strong_observer.strong_observe();
                }
            },
            || {},
        );
    } else {
        run_bench_threads(
            BenchThreadConfig {
                n_threads: n_threads_per_type.wrapping_sub(1),
                n_messages: 2 * N_MESSAGES_PER_THREAD,
                barrier: barrier.clone(),
                completed: completed.clone(),
                label: "strong_observer",
                cpu_offset: n_threads_per_type + 2,
            },
            || {
                let strong_observer = q.attach_consumer().unwrap();
                move || {
                    let _ = strong_observer.consume();
                }
            },
            || {},
        );
    }
}

fn strong_observer_bench<Q: ConsumableOQueue<u64>>(
    input: &OQueueBenchmarkInput,
    q: &Arc<Q>,
    completed: &Arc<AtomicUsize>,
) {
    assert!(
        input.n_threads.is_multiple_of(2),
        "strong_observer_bench: bench.n_threads must be even (got {})",
        input.n_threads
    );
    let n_threads_per_type: usize = input.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(input.n_threads));

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type,
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "producer",
            cpu_offset: 1,
        },
        || {
            let producer = q.attach_value_producer().unwrap();
            move || {
                producer.produce(0);
            }
        },
        || {},
    );

    spawn_bench_thread(
        n_threads_per_type + 1,
        barrier.clone(),
        completed.clone(),
        "consumer",
        false,
        {
            let consumer = q.attach_consumer().unwrap();
            move || {
                for _ in 0..(2 * N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
            }
        },
    );

    run_bench_threads(
        BenchThreadConfig {
            n_threads: n_threads_per_type.wrapping_sub(1),
            n_messages: 2 * N_MESSAGES_PER_THREAD,
            barrier: barrier.clone(),
            completed: completed.clone(),
            label: "strong_observer",
            cpu_offset: n_threads_per_type + 2,
        },
        || {
            let strong_observer = q
                .attach_strong_observer(ObservationQuery::new(|v: &u64| *v))
                .unwrap();
            move || {
                let _ = strong_observer.strong_observe();
            }
        },
        || {},
    );
}

type OQueueBenchFn =
    &'static dyn Fn(&OQueueLegacyBenchmarkInput, &Arc<dyn OQueue<u64>>, &Arc<AtomicUsize>);

struct OQueueLegacyBenchmark {
    fn_: OQueueBenchFn,
    name: String,
    input: Option<OQueueLegacyBenchmarkInput>,
}

impl OQueueLegacyBenchmark {
    fn new(fn_: OQueueBenchFn, name: &str) -> Box<Self> {
        let name = name.to_string();
        Box::new(Self {
            fn_,
            name,
            input: None,
        })
    }

    fn get_oqueue(&self) -> Arc<dyn OQueue<u64>> {
        let input = self.input.as_ref().unwrap();
        let q_type = &input.q_type;
        let n_messages = input.n_messages;

        if q_type == "mpmc_oq" {
            let q = crate::benchmarks::legacy_oqueue::ringbuffer::mpmc::MPMCOQueue::<u64>::new(
                2 << 20,
                16,
            );
            assert!(q.capacity() >= n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else if q_type == "locking" {
            let q = crate::benchmarks::legacy_oqueue::locking::ObservableLockingQueue::<u64>::new(
                2 << 20,
                16,
            );
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

impl Benchmark for OQueueLegacyBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        let karg = get_kernel_cmd_line().expect("no kernel command line");
        self.input = Some(OQueueLegacyBenchmarkInput {
            n_threads,
            n_messages: N_MESSAGES_PER_THREAD * n_threads,
            q_type: karg
                .get_module_arg_by_name::<String>("bench", "q_type")
                .expect("missing bench.q_type=... on kernel command line"),
        });
    }

    fn run(&self, completed: Arc<AtomicUsize>) {
        let q: Arc<dyn OQueue<u64>> = self.get_oqueue();
        let input = self.input.as_ref().unwrap();
        (self.fn_)(input, &q, &completed);
    }

    fn name(&self) -> &str {
        &self.name
    }
}

enum OQueueScalingBenchmarkType {
    Consumer,
    StrongObserver,
    WeakObserver,
}

struct OQueueScalingBenchmark {
    name: String,
    test_type: OQueueScalingBenchmarkType,
    n_threads: usize,
}

impl OQueueScalingBenchmark {
    fn new(name: &str, test_type: OQueueScalingBenchmarkType) -> Box<Self> {
        let name = name.to_string();
        Box::new(Self {
            name,
            test_type,
            n_threads: 0,
        })
    }
}

impl Benchmark for OQueueScalingBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        self.n_threads = n_threads;
    }
    // large number of producers pushing a fixed # of msgs with:
    //  1 consumer + 0 strong observer + 0 weak observer
    //  0 consumer + 1 strong observer + 0 weak observer
    //  0 consumer + 0 strong observer + 1 weak observer
    // measure producer throughput (and latency?)

    fn run(&self, completed: Arc<AtomicUsize>) {
        let n_threads = self.n_threads;
        let n_producers: usize = n_threads - 1;

        const N_MESSAGES_PER_PRODUCER: usize = 1024 * 1024;

        fn producer_thread(q: Box<dyn Producer<()>>) {
            for _ in 0..N_MESSAGES_PER_PRODUCER {
                q.produce(());
            }
        }
        let consumer_thread = move |q: Vec<Box<dyn Consumer<()>>>| {
            let mut n_recv = 0;
            while n_recv < (n_producers * N_MESSAGES_PER_PRODUCER) {
                for c in &q {
                    if c.try_consume().is_some() {
                        n_recv += 1;
                    }
                }
            }
        };
        let strong_observer_thread = move |q: Vec<Box<dyn StrongObserver<()>>>| {
            let mut n_recv = 0;
            while n_recv < (n_producers * N_MESSAGES_PER_PRODUCER) {
                for c in &q {
                    if c.try_strong_observe().is_some() {
                        n_recv += 1;
                    }
                }
            }
        };
        let weak_observer_thread = {
            let completed = completed.clone();
            move |q: Vec<Box<dyn WeakObserver<()>>>| {
                let mut cursors = Vec::<Cursor>::new();
                for c in &q {
                    cursors.push(c.oldest_cursor());
                }
                let mut n_recv = 0;
                while completed.load(Ordering::Relaxed) < n_producers
                    && n_recv < (n_producers * N_MESSAGES_PER_PRODUCER)
                {
                    for (c, cursor) in q.iter().zip(cursors.iter_mut()) {
                        while c.weak_observe(*cursor).is_some() {
                            *cursor += 1;
                            n_recv += 1;
                        }
                        *cursor = c.recent_cursor();
                    }
                }
                println!(
                    "Weak Observer, observed {}/{} msgs",
                    n_recv,
                    (n_producers * N_MESSAGES_PER_PRODUCER)
                );
            }
        };

        println!("Starting producers");
        let barrier = Arc::new(AtomicUsize::new(n_threads));

        let mut queues: Vec<Arc<dyn OQueue<()>>> = Vec::new();
        for _ in 0..n_producers {
            queues.push(MPMCOQueue::<(), true, true>::new(1024, 1));
        }

        for (tid, q) in queues.iter().enumerate() {
            let producer = q.attach_producer().unwrap();
            spawn_bench_thread(
                tid + 1,
                barrier.clone(),
                completed.clone(),
                "producer",
                false,
                move || producer_thread(producer),
            );
        }

        match self.test_type {
            OQueueScalingBenchmarkType::Consumer => {
                let handles = queues
                    .iter()
                    .map(|q| q.attach_consumer().unwrap())
                    .collect();
                spawn_bench_thread(
                    n_threads,
                    barrier.clone(),
                    completed.clone(),
                    "consumer",
                    true,
                    move || consumer_thread(handles),
                );
            }
            OQueueScalingBenchmarkType::StrongObserver => {
                let handles = queues
                    .iter()
                    .map(|q| q.attach_strong_observer().unwrap())
                    .collect();
                spawn_bench_thread(
                    n_threads,
                    barrier.clone(),
                    completed.clone(),
                    "strong_observer",
                    true,
                    move || strong_observer_thread(handles),
                );
            }
            _ => {
                let handles = queues
                    .iter()
                    .map(|q| q.attach_weak_observer().unwrap())
                    .collect();
                spawn_bench_thread(
                    n_threads,
                    barrier.clone(),
                    completed.clone(),
                    "weak_observer",
                    true,
                    move || weak_observer_thread(handles),
                );
            }
        }
    }
    fn name(&self) -> &str {
        &self.name
    }
}

struct OQueueBenchmark {
    name: String,
    test_type: BenchmarkType,
    input: Option<OQueueBenchmarkInput>,
}

struct OQueueBenchmarkInput {
    pub n_threads: usize,
}

enum BenchmarkType {
    Produce,
    Consume,
    Mixed,
    WeakObserver,
    StrongObserver,
}

impl OQueueBenchmark {
    fn new(name: &str, test_type: BenchmarkType) -> Box<Self> {
        Box::new(Self {
            name: name.to_string(),
            input: None,
            test_type,
        })
    }

    fn get_ref_oqueue(&self) -> OQueueRef<u64> {
        OQueueRef::new_anonymous(2 << 20)
    }

    fn get_consumable_oqueue(&self) -> ConsumableOQueueRef<u64> {
        ConsumableOQueueRef::new_anonymous(2 << 20)
    }
}

impl Benchmark for OQueueBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        let _karg = get_kernel_cmd_line().expect("no kernel command line");
        self.input = Some(OQueueBenchmarkInput { n_threads });
    }

    fn run(&self, completed: Arc<AtomicUsize>) {
        let input = self.input.as_ref().unwrap();
        match self.test_type {
            BenchmarkType::Produce => {
                let q = Arc::new(self.get_ref_oqueue());
                produce_bench(input, &q, &completed);
            }
            BenchmarkType::Consume => {
                let q = Arc::new(self.get_consumable_oqueue());
                consume_bench(input, &q, &completed);
            }
            BenchmarkType::Mixed => {
                let q = Arc::new(self.get_consumable_oqueue());
                mixed_bench(input, &q, &completed);
            }
            BenchmarkType::WeakObserver => {
                let q = Arc::new(self.get_consumable_oqueue());
                weak_observer_bench(input, &q, &completed);
            }
            BenchmarkType::StrongObserver => {
                let q = Arc::new(self.get_consumable_oqueue());
                strong_observer_bench(input, &q, &completed);
            }
        }
    }
    fn name(&self) -> &str {
        &self.name
    }
}

pub fn register_benchmarks(bc: &mut BenchmarkHarness) {
    bc.register_benchmark(OQueueLegacyBenchmark::new(
        &produce_bench_legacy,
        "oqueue::produce_bench_legacy",
    ));
    bc.register_benchmark(OQueueLegacyBenchmark::new(
        &consume_bench_legacy,
        "oqueue::consume_bench_legacy",
    ));
    bc.register_benchmark(OQueueLegacyBenchmark::new(
        &mixed_bench_legacy,
        "oqueue::mixed_bench_legacy",
    ));
    bc.register_benchmark(OQueueLegacyBenchmark::new(
        &weak_observer_bench_legacy,
        "oqueue::weak_observer_bench_legacy",
    ));
    bc.register_benchmark(OQueueLegacyBenchmark::new(
        &strong_observer_bench_legacy,
        "oqueue::strong_observer_bench_legacy",
    ));

    bc.register_benchmark(OQueueScalingBenchmark::new(
        "oqueue_scaling::consumer",
        OQueueScalingBenchmarkType::Consumer,
    ));
    bc.register_benchmark(OQueueScalingBenchmark::new(
        "oqueue_scaling::strong_observer",
        OQueueScalingBenchmarkType::StrongObserver,
    ));
    bc.register_benchmark(OQueueScalingBenchmark::new(
        "oqueue_scaling::weak_observer",
        OQueueScalingBenchmarkType::WeakObserver,
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        "oqueue::produce_bench",
        BenchmarkType::Produce,
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        "oqueue::consume_bench",
        BenchmarkType::Consume,
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        "oqueue::mixed_bench",
        BenchmarkType::Mixed,
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        "oqueue::weak_observer_bench",
        BenchmarkType::WeakObserver,
    ));
    bc.register_benchmark(OQueueBenchmark::new(
        "oqueue::strong_observer_bench",
        BenchmarkType::StrongObserver,
    ));
}
