// SPDX-License-Identifier: MPL-2.0
//! A wait queue implementation based on the waker/waiter utilities.

use alloc::{collections::VecDeque, sync::Arc};
use core::sync::atomic::{AtomicU32, Ordering};

use super::{LocalIrqDisabled, SpinLock};
use crate::sync::{wait_mechanism::WaitMechanism, Waiter, Waker};

/// A wait queue.
///
/// One may wait on a wait queue to put its executing thread to sleep.
/// Multiple threads may be the waiters of a wait queue.
/// Other threads may invoke the `wake`-family methods of a wait queue to
/// wake up one or many waiting threads.
pub struct WaitQueue {
    // A copy of `wakers.len()`, used for the lock-free fast path in `wake_one` and `wake_all`.
    num_wakers: AtomicU32,
    wakers: SpinLock<VecDeque<Arc<Waker>>, LocalIrqDisabled>,
}

impl WaitQueue {
    /// Creates a new, empty wait queue.
    pub const fn new() -> Self {
        WaitQueue {
            num_wakers: AtomicU32::new(0),
            wakers: SpinLock::new(VecDeque::new()),
        }
    }

    /// Waits until some condition is met.
    ///
    /// This method takes a closure that tests a user-given condition.
    /// The method only returns if the condition returns `Some(_)`.
    /// A waker thread should first make the condition `Some(_)`, then invoke the
    /// `wake`-family method. This ordering is important to ensure that waiter
    /// threads do not lose any wakeup notifications.
    ///
    /// By taking a condition closure, this wait-wakeup mechanism becomes
    /// more efficient and robust.
    #[track_caller]
    pub fn wait_until<F, R>(&self, mut cond: F) -> R
    where
        F: FnMut() -> Option<R>,
    {
        if let Some(res) = cond() {
            return res;
        }

        let (waiter, _) = Waiter::new_pair();
        let cond = || {
            self.enqueue(waiter.waker());
            cond()
        };
        waiter
            .wait_until_or_cancelled(cond, || Ok::<(), ()>(()))
            .unwrap()
    }

    /// Wakes up one waiting thread, if there is one at the point of time when this method is
    /// called, returning whether such a thread was woken up.
    pub fn wake_one(&self) -> bool {
        // Fast path
        if self.is_empty() {
            return false;
        }

        loop {
            let mut wakers = self.wakers.lock();
            let Some(waker) = wakers.pop_front() else {
                return false;
            };
            self.num_wakers.fetch_sub(1, Ordering::Release);
            // Avoid holding lock when calling `wake_up`
            drop(wakers);

            if waker.wake_up() {
                return true;
            }
        }
    }

    /// Wakes up all waiting threads, returning the number of threads that were woken up.
    pub fn wake_all(&self) -> usize {
        // Fast path
        if self.is_empty() {
            return 0;
        }

        let mut num_woken = 0;

        loop {
            let mut wakers = self.wakers.lock();
            let Some(waker) = wakers.pop_front() else {
                break;
            };
            self.num_wakers.fetch_sub(1, Ordering::Release);
            // Avoid holding lock when calling `wake_up`
            drop(wakers);

            if waker.wake_up() {
                num_woken += 1;
            }
        }

        num_woken
    }

    fn is_empty(&self) -> bool {
        // On x86-64, this generates `mfence; mov`, which is exactly the right way to implement
        // atomic loading with `Ordering::Release`. It performs much better than naively
        // translating `fetch_add(0)` to `lock; xadd`.
        self.num_wakers.fetch_add(0, Ordering::Release) == 0
    }

    /// Enqueues the input [`Waker`] to the wait queue.
    #[doc(hidden)]
    pub fn enqueue(&self, waker: Arc<Waker>) {
        let mut wakers = self.wakers.lock();
        wakers.push_back(waker);
        self.num_wakers.fetch_add(1, Ordering::Acquire);
    }
}

impl WaitMechanism for WaitQueue {
    fn wait_until<F, R>(&self, cond: F) -> R
    where
        F: FnMut() -> Option<R>,
    {
        self.wait_until(cond)
    }

    fn enqueue(&self, waker: Arc<Waker>) {
        self.enqueue(waker);
    }

    fn wake_one(&self) {
        self.wake_one();
    }

    fn wake_all(&self) {
        self.wake_all();
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}



#[cfg(ktest)]
mod test {
    use core::sync::atomic::AtomicBool;

    use super::*;
    use crate::{prelude::*, task::{Task, TaskOptions}};

    fn queue_wake<F>(wake: F)
    where
        F: Fn(&WaitQueue) + Sync + Send + 'static,
    {
        let queue = Arc::new(WaitQueue::new());
        let queue_cloned = queue.clone();

        let cond = Arc::new(AtomicBool::new(false));
        let cond_cloned = cond.clone();

        TaskOptions::new(move || {
            Task::yield_now();

            cond_cloned.store(true, Ordering::Relaxed);
            wake(&queue_cloned);
        })
        .data(())
        .spawn()
        .unwrap();

        queue.wait_until(|| cond.load(Ordering::Relaxed).then_some(()));

        assert!(cond.load(Ordering::Relaxed));
    }

    #[ktest]
    fn queue_wake_one() {
        queue_wake(|queue| {
            queue.wake_one();
        });
    }

    #[ktest]
    fn queue_wake_all() {
        queue_wake(|queue| {
            queue.wake_all();
        });
    }
}
