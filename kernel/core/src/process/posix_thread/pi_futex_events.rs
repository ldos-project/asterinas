// SPDX-License-Identifier: MPL-2.0

//! The `futex.pi.events` OQueue, which reports how threads contend on PI futexes.
//!
//! Together with `scheduler.events`, this lets a policy see who waits on whom: a `Block` event
//! names a waiter and the owner it is stuck behind, along with both of their scheduling policies,
//! so a high-priority thread blocked behind a low-priority one (a priority inversion) shows up
//! directly. `WaitEnd` and `Handoff` events then report how long the wait lasted and who the lock
//! went to.
//!
//! Events are produced best-effort: if the queue is full, the event is dropped rather than
//! stalling the locking thread.

use serde::Serialize;

use crate::{
    prelude::*,
    process::Pid,
    sched::SchedPolicyView,
    thread::{Thread, Tid},
    time::clocks::MonotonicClock,
};

/// An event on a PI futex.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PiFutexEvent {
    /// The monotonic time of the event, in nanoseconds.
    pub time_ns: u64,
    /// The process whose address space holds the futex.
    pub pid: Pid,
    /// The user-space address of the futex word.
    pub futex_addr: u64,
    pub kind: PiFutexEventKind,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum PiFutexEventKind {
    /// `waiter` started to wait for the futex held by `owner`.
    Block {
        waiter: Tid,
        waiter_policy: SchedPolicyView,
        owner: Tid,
        owner_policy: SchedPolicyView,
    },
    /// `waiter` stopped waiting after `waited_ns` nanoseconds.
    WaitEnd {
        waiter: Tid,
        waited_ns: u64,
        outcome: PiWaitOutcome,
    },
    /// `from` released the futex directly to the waiter `to`, leaving `waiters_left` other
    /// threads waiting.
    Handoff {
        from: Tid,
        to: Tid,
        waiters_left: u32,
    },
}

/// Why a PI futex waiter stopped waiting.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum PiWaitOutcome {
    /// The waiter was handed the lock.
    Acquired,
    /// The waiter was woken without the lock (e.g., the owner died) and will try again.
    Retry,
    TimedOut,
    Interrupted,
}

impl PiFutexEvent {
    pub fn new(futex_addr: Vaddr, kind: PiFutexEventKind) -> Self {
        Self {
            time_ns: now_ns(),
            pid: current!().pid(),
            futex_addr: futex_addr as u64,
            kind,
        }
    }
}

/// Returns the current monotonic time in nanoseconds.
pub fn now_ns() -> u64 {
    MonotonicClock::get().read_time().as_nanos() as u64
}

pub fn policy_of(thread: &Thread) -> SchedPolicyView {
    thread.sched_attr().policy().into()
}

#[cfg(not(baseline_asterinas))]
mod imp {
    use ostd::{
        orpc::oqueue::{GenericOQueueRef, OQueue as _, OQueueBase as _, RefProducer, registry},
        path,
    };
    use spin::Once;

    use super::PiFutexEvent;

    static PRODUCER: Once<RefProducer<PiFutexEvent>> = Once::new();

    pub fn init() {
        PRODUCER.call_once(|| {
            let path = path!(futex.pi.events);
            let oqueue = GenericOQueueRef::new(1024, path.clone());
            // Export the queue under the OQueue filesystem (`/oqueues/futex/pi/events`).
            registry::register(&path, &oqueue.as_any_oqueue());
            oqueue
                .attach_ref_producer()
                .expect("a new OQueue always allows a ref producer")
        });
    }

    pub fn produce(event: impl FnOnce() -> PiFutexEvent) {
        if let Some(producer) = PRODUCER.get() {
            // Drop the event if the queue is full; never block a locking thread on an observer.
            let _ = producer.try_produce_ref(&event());
        }
    }
}

#[cfg(baseline_asterinas)]
mod imp {
    use super::PiFutexEvent;

    pub fn init() {}

    pub fn produce(_event: impl FnOnce() -> PiFutexEvent) {}
}

pub use imp::{init, produce};
