// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc};

use ostd::orpc::{
    framework::{Server, inject_spawn_thread},
    oqueue::{OQueueRef, ringbuffer::MPMCOQueue},
};

use crate::thread::kernel_thread::ThreadOptions;

// Constructor for a new OQueue. This is to make testing easier to switch between oqueue
// implementations.
pub fn new_oqueue<T: Copy + Send + 'static>() -> OQueueRef<T> {
    // A locking version of OQueue can be enabled by uncommenting the following:
    // ostd::orpc::oqueue::locking::ObservableLockingQueue::new(8, 8)
    MPMCOQueue::<T, true, true>::new(8, 8)
}

// Constructor for a new OQueue which needs a specific length. This is needed for cases where a long
// queue is required to avoid deadlocks.
pub fn new_oqueue_with_len<T: Copy + Send + 'static>(len: usize) -> OQueueRef<T> {
    // A locking version of OQueue can be enabled by uncommenting the following:
    // ostd::orpc::oqueue::locking::ObservableLockingQueue::new(len, 8)
    MPMCOQueue::<T, true, true>::new(len, 8)
}

// NOTE: This cannot be ktested, because this is only meaningful within the full kernel environment.

/// Start a new server thread. This should only be called while spawning a server.
fn spawn_thread(
    server: Arc<dyn Server + Send + Sync + 'static>,
    body: Box<dyn (FnOnce() -> Result<(), Box<dyn core::error::Error>>) + Send + 'static>,
) {
    let _ = ThreadOptions::new(ostd::orpc::framework::wrap_server_thread_body(server, body))
        // TODO(arthurp): This sets server threads to be real-time threads with a medium priority. This
        // prevents them from being blocked by user threads, but is probably not the right solution in
        // general.
        .sched_policy(crate::sched::SchedPolicy::RealTime {
            rt_prio: 50.try_into().unwrap(),
            rt_policy: Default::default(),
        })
        .spawn();
}

pub fn init() {
    inject_spawn_thread(spawn_thread);
}
