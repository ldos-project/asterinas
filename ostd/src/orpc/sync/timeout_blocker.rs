// SPDX-License-Identifier: MPL-2.0

//! A [`Blocker`] that becomes ready after a timeout.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use super::Blocker;
use crate::{
    sync::{WaitQueue, Waker, WakerKey},
    timer::{self, Jiffies},
};

/// A [`Blocker`] that becomes ready once an armed jiffies deadline elapses.
///
/// Add it to a wait set (e.g. [`BlockOnMany::block_on`]), potentially alongside other
/// blockers, so a thread that is waiting for events can also be woken
/// by a timeout. It could be used to build loops that want to block until
/// *either* a message arrives *or* a deadline passes.
///
/// It starts disarmed and can be armed and disarmed repeatedly:
/// - [`arm_at`](Self::arm_at) / [`arm_after`](Self::arm_after) set the deadline.
/// - [`disarm`](Self::disarm) makes it never fire.
///
/// # Implementation and caveats
///
/// [`new`](Self::new) registers a per-tick timer callback (via
/// [`timer::register_callback_on_cpu`]) that wakes the blocker's waiters once the
/// deadline passes. That callback lives as long as the process runs and **cannot
/// be unregistered**, so this type is intended for long-lived blockers rather
/// than short-lived, frequently-created ones. Once the `TimeoutBlocker` is
/// dropped, the callback becomes an inexpensive no-op (it holds only a weak
/// reference and does one atomic load per tick).
///
/// [`BlockOnMany::block_on`]: super::BlockOnMany::block_on
pub struct TimeoutBlocker {
    /// Absolute jiffies deadline; `u64::MAX` means disarmed.
    deadline: AtomicU64,
    /// Waiters to wake once the deadline elapses.
    wait_queue: WaitQueue,
}

impl TimeoutBlocker {
    /// Creates a new, disarmed `TimeoutBlocker`.
    ///
    /// The value is returned inside an [`Arc`] because the internal timer
    /// callback holds a weak reference to it.
    pub fn new() -> Arc<Self> {
        let this = Arc::new(Self {
            deadline: AtomicU64::new(u64::MAX),
            wait_queue: WaitQueue::new(),
        });

        // Wake the registered waiters once the deadline elapses. The callback
        // holds only a `Weak` reference, so it becomes a no-op if the blocker is
        // dropped. 
        let weak = Arc::downgrade(&this);
        timer::register_callback_on_cpu(move || {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if Jiffies::elapsed().as_u64() >= this.deadline.load(Ordering::Relaxed) {
                this.wait_queue.wake_all();
            }
        });

        this
    }

    /// Arms the timeout to fire at the absolute jiffies value `deadline`.
    pub fn arm_at(&self, deadline: u64) {
        self.deadline.store(deadline, Ordering::Relaxed);
    }

    /// Arms the timeout to fire `jiffies` jiffies from now.
    pub fn arm_after(&self, jiffies: u64) {
        let deadline = Jiffies::elapsed().as_u64().saturating_add(jiffies);
        self.deadline.store(deadline, Ordering::Relaxed);
    }

    /// Disarms the timeout so it never fires.
    pub fn disarm(&self) {
        self.deadline.store(u64::MAX, Ordering::Relaxed);
    }
}

impl Blocker for TimeoutBlocker {
    fn should_try(&self) -> bool {
        Jiffies::elapsed().as_u64() >= self.deadline.load(Ordering::Relaxed)
    }

    fn enqueue(&self, waker: &Arc<Waker>) -> WakerKey {
        self.wait_queue.enqueue(waker.clone())
    }

    fn remove(&self, key: WakerKey) {
        self.wait_queue.remove(key)
    }
}
