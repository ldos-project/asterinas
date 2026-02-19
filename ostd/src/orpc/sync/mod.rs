// SPDX-License-Identifier: MPL-2.0

//! A trait [`Blocker`] which allows a thread to wait for a wake-up from another thread. The API is designed to allow a
//! waiter to wait on multiple blockers at the same time to support [`select!`].

pub use orpc_macros::select;

use crate::{
    orpc::framework::CurrentServer,
    prelude::Arc,
    sync::{Waiter, Waker, WakerKey},
    task::{CurrentTask, Task},
};

// TODO(#73): This will need rework since it is inefficient and not well architected. It probably needs to use a more event
// based approach more like epoll. That would imply attaching a callback to the waker so that it can provide information
// on what event(s) actually occurred back to the waiter. This will be even more complex, but may provide better
// performance.
//
// NOTE: This avoids rechecks on the blocking OQueues. Those rechecks have atomic operations that can contend on other
// users of the OQueue. HOWEVER, the contention may occur exactly when a wake up is actually required meaning that the
// cost may be very low.

/// Tasks can block waiting for a [`Blocker`] to notify them to retry an action.
///
/// User code should generally only call [`Blocker::block_until`] or [`CurrentTask::block_on`].
pub trait Blocker {
    /// Return true if performing the action may succeed and should be attempted. This must be *very* fast and cannot
    /// block for any condition itself. This is because it will be called inside the scheduler with locks held.
    ///
    /// This should be an approximation of the success of a `try_` function such as [`Consumer::try_produce`]. This
    /// *must* return true if `try`ing would succeed, but may also return true spuriously even if it will fail.
    ///
    /// This must have Acquire ordering.
    fn should_try(&self) -> bool;

    /// Add a task to the wait queue of `self`. After this call, the task must be awoken if [`Self::should_try`] may
    /// return `true` again.
    ///
    /// This must have Release ordering.
    ///
    /// This returns an ID which can be passed to [`Self::remove`] (on the same instance) to improve the
    /// performance of removal.
    fn enqueue(&self, waker: &Arc<Waker>) -> WakerKey;

    /// Remove a task from the wait queue of self. After this call, the task will not be awoken when
    /// when `should_try` will return true. This undoes the effect of [`Self::enqueue`].
    fn remove(&self, key: WakerKey);

    /// Block on self repeately until `cond` returns Some. This assumes that this blocker will be woken if `cond()`
    /// would change.
    ///
    /// To block on multiple `Blockers`, use [`CurrentTask::block_on`].
    fn block_until<T>(&self, cond: impl Fn() -> Option<T>) -> T
    where
        Self: Sized,
    {
        loop {
            let ret = cond();
            match ret {
                Some(returned) => {
                    return returned;
                }
                None => {
                    if let Some(t) = Task::current() {
                        t.block_on(&[self]);
                    }
                }
            };
        }
    }
}

// Optional Blockers are allowed. If the blocker is `None`, then it will never wake up and performs
// no other handling.
impl<T: Blocker> Blocker for Option<T> {
    fn should_try(&self) -> bool {
        self.as_ref().is_some_and(T::should_try)
    }

    fn enqueue(&self, waker: &Arc<Waker>) -> WakerKey {
        if let Some(blocker) = self {
            blocker.enqueue(waker)
        } else {
            WakerKey::default()
        }
    }

    fn remove(&self, key: WakerKey) {
        if let Some(blocker) = self {
            blocker.remove(key)
        }
    }
}

impl CurrentTask {
    /// Wait for multiple blockers, waking if any wake. This is equivalent to
    /// [`Blocker::block_until`] if there is only one blocker.
    pub fn block_on<const N: usize>(&self, blockers: &[&dyn Blocker; N]) {
        CurrentServer::abort_point();
        let (waiter, waker) = Waiter::new_pair();

        // 1. Register for all blockers.
        let keys = blockers.map(|blocker| blocker.enqueue(&waker));

        // 2. Check if any of the blockers are actually ready.
        if !blockers.iter().any(|b| b.should_try()) {
            // 3. Block if no blockers are ready. This will immediately wake if any blocker woke
            //    between `prepare_to_wait` and here. This prevents this thread from dropping a wake
            //    between `should_try` and `wait_cancellable`.
            waiter.wait_cancellable(|| {
                if CurrentServer::is_aborted() {
                    Err(())
                } else {
                    Ok(())
                }
            });
        }

        // 4. Unregister with all the blockers. This avoids the blocker queues growing without
        //    bound.
        for (blocker, key) in blockers.iter().zip(keys) {
            blocker.remove(key);
        }

        CurrentServer::abort_point();
    }
}
