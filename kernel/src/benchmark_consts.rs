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
    panic::{RefUnwindSafe, UnwindSafe},
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

use super::*;

/// A single element (slot for storing a value) in a ring buffer.
#[derive(Debug)]
struct Element<T> {
    /// The data stored in this element of the ring buffer. This is value is initialized if either the valid bit in
    /// `weak_reader_states` is set or this element is between the head (read) and tail (write) indexes of the ring
    /// buffer. This assumes correct synchronization using the various atomic values used by the ring buffer.
    data: UnsafeCell<MaybeUninit<T>>,
}
// TODO(aneesh)
impl<T> UnwindSafe for Element<T> {}
impl<T> RefUnwindSafe for Element<T> {}

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
        unsafe {
            let data = ptr::read(self.value.data.get()).assume_init();
            data
        }
    }

    #[expect(unused)]
    unsafe fn get_maybe_uninit(&self) -> MaybeUninit<T> {
        // SAFETY: This assumes that the self.store has be previously called
        unsafe {
            let data = ptr::read(self.value.data.get());
            data
        }
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
    /// new
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

    ///
    pub fn size(&self) -> usize {
        self.head.load(Ordering::Relaxed) - self.tail.load(Ordering::Relaxed)
    }

    ///
    #[expect(unused)]
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

//pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;
pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;
pub struct BenchConsts {
    pub n_threads: usize,
    pub n_messages: usize,
    pub q_type: String,
    pub benchmark: String,
}
impl BenchConsts {
    pub fn new(karg: &super::KCmdlineArg) -> Self {
        let n_threads = karg
            .get_module_arg_by_name::<usize>("bench", "n_threads")
            .unwrap();
        Self {
            n_threads,
            n_messages: N_MESSAGES_PER_THREAD * n_threads,
            q_type: karg
                .get_module_arg_by_name::<String>("bench", "q_type")
                .unwrap(),
            benchmark: karg
                .get_module_arg_by_name::<String>("bench", "benchmark")
                .unwrap(),
        }
    }

    pub fn get_oq(&self) -> Arc<dyn OQueue<u64>> {
        if self.q_type == "mpmc_oq" {
            let q = ostd::orpc::oqueue::ringbuffer::mpmc::MPMCOQueue::<u64>::new(2 << 20, 16);
            assert!(q.capacity() >= self.n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else if self.q_type == "locking" {
            let q = ostd::orpc::oqueue::locking::ObservableLockingQueue::<u64>::new(2 << 20, 16);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else {
            let q = Rigtorp::<u64>::new(2 << 20);
            assert!(q.capacity() >= self.n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        }
    }

    pub fn run_benchmark(&self) {
        let benchmark = match self.benchmark.as_str() {
            "mixed_bench" => benchmarks::mixed_bench,
            "consume_bench" => benchmarks::consume_bench,
            "produce_bench" => benchmarks::produce_bench,
            "weak_obs_bench" => benchmarks::weak_obs_bench,
            "strong_obs_bench" => benchmarks::strong_obs_bench,
            _ => panic!("Unknown benchmark"),
        };
        for _ in 0..10 {
            let now = time::clocks::RealTimeClock::get().read_time();

            let q: Arc<dyn OQueue<u64>> = self.get_oq();

            let completed = Arc::new(AtomicUsize::new(0));
            let completed_wq = Arc::new(ostd::sync::WaitQueue::new());
            let n_threads = benchmark(self, &q, &completed, &completed_wq);

            println!("Waiting for benchmark to complete");
            // Exit after benchmark completes
            while completed.load(Ordering::Relaxed) != n_threads {
                core::hint::spin_loop();
            }
            let end = time::clocks::RealTimeClock::get().read_time();

            println!("[total] {:?}", end - now);
        }
    }
}
