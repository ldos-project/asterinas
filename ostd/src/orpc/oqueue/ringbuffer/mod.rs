use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    panic::{RefUnwindSafe, UnwindSafe},
    ptr,
};

pub mod mpmc;
pub mod spsc;

pub use mpmc::{MPMCConsumer, MPMCOQueue, MPMCProducer, MPMCStrongObserver, MPMCWeakObserver};
pub use spsc::{SPSCConsumer, SPSCOQueue, SPSCProducer, SPSCStrongObserver, SPSCWeakObserver};

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
