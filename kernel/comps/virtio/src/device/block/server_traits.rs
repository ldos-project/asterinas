use alloc::boxed::Box;

use ostd::orpc::{
    oqueue::{
        OQueue as _, OQueueRef, Producer, locking::ObservableLockingQueue, reply::ReplyQueue,
    },
    orpc_trait,
};

#[orpc_trait]
pub trait PageIOObservable {
    /// The OQueue containing every read request. This includes both sync and async reads on this
    /// trait and any other read operations on other traits (for instance,
    /// [`crate::vm::vmo::Pager::commit_page`]).
    fn page_reads_oqueue(&self) -> OQueueRef<usize> {
        ObservableLockingQueue::new(8, 8)
    }

    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn page_writes_oqueue(&self) -> OQueueRef<usize> {
        ObservableLockingQueue::new(8, 8)
    }
}
