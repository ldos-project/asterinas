//! A trait [`Blocker`] which allows a thread to wait for a wake-up from another thread. The API is designed to allow a
//! waiter to wait on multiple blockers at the same time to support [`select!`].
//! 
//! XXX: This needs to be reworked significantly because is forces some rather odd syntax in select and is not as
//! flexible as it should be.

use std::{mem, sync::Mutex};

use crate::{
    orpc_impl::framework::CurrentServer,
    sync::task::{CurrentTask, Task, TaskState},
};

use super::task;

pub use orpc_macros::select;

// XXX: This will need rework since it is inefficient and not well architected. It probably needs to use a more event
// based approach more like epoll. That would imply attaching a callback to the waker so that it can provide information
// on what event(s) actually occurred back to the waiter. This will be even more complex, but may provide better
// performance.
//
// NOTE: This avoids rechecks on the blocking OQueues. Those rechecks have atomic operations that can contend on other
// users of the OQueue. HOWEVER, the contention may occur exactly when a wake up is actually required meaning that the
// cost may be very low.

// XXX: The current setup for blocking is to include a call to `try_*` and infer the blocker from the receiver. This is
// kind of "magical". It would be better to have a type which encapsulates the async call information (the try function
// and the blocker). This becomes extremely similar to the `Future` types in Rust async. However, it performs no
// computation when the check is made, only doing a few instructions to check if the operation is possible. This is
// critical as the check must occur with the thread in a special scheduling state.

/// Tasks can block waiting for a [`Blocker`] to notify them to retry an action.
///
/// To use this, the scheduler will:
///
/// 1. Lock the task (spinning) and disable preemption.
/// 2. Use `add_task` to register the task to be awoken when the blocker unblocks.
/// 3. Call `should_try` to check if it should actually block.
/// 4. If the task should try:
///         1. Unregister the task with `remove_task`.
///         2. Unlock the task into the running state.
///
///    If the task should not try:
///         1. Unlock the task into the blocked state.
///
/// To block on multiple blockers:
///
/// 1. Lock the task (spinning) and disable preemption.
/// 2. For each blocker:
///     1. Use `add_task` to register the task to be awoken when the blocker unblocks.
///     2. Call `should_try` to check if it should actually block.
/// 4. If the task should try:
///         1. Unregister the task from all blockers registered so far with `remove_task`.
///         2. Unlock the task into the running state.
///
///    If the task should not try:
///         1. Unlock the task into the blocked state.
///
/// To wake tasks the blocker will iterate the tasks and for each: (The waker must atomically "take" the list,
/// guanteeing that exactly one waker gets the non-empty list.)
///
/// 1. Lock the task. (Spinning)
/// 2. Unlock the task into the runnable state and place it into the run queue.
///
/// As written, this does not allow for `wake_one`, only `wake_all`. This is because we have no way to know if a task
/// will actually "try" the action after it is woken. This could fail because the task could have been woken already and
/// already passed the point where it would perform the check. This could happen in an ABA situation as well, where the
/// thread has blocked again, but waiting for a different blocker.
///
/// These wait semantics also force every blocker to be checked everytime a task is awoken. This is because multiple
/// wakes could have occurred from different blockers. These is no way to distinguish multiple wakes from a single.
///
/// NOTE: Many requirements here can be relaxed in cases where there is guaranteed to be only one waker thread or
/// similar limitations. This *may* improve performance, but may not. An obvious case would be single sender queues
/// not requiring an atomic take operation on the wait queue.
pub trait Blocker {
    /// Return true if performing the action may succeed and should be attempted. This must be *very* fast and cannot
    /// block for any condition itself. This is because it will be called inside the scheduler with locks held.
    ///
    /// This should be an approximation of the success of a `try_` function such as [`Receiver::try_receive`]. This
    /// *must* return true if `try`ing would succeed, but may also return true spuriously even if it will fail.
    ///
    /// This must have Acquire ordering.
    fn should_try(&self) -> bool;

    /// Add a task to the wait queue of `self`. After this call, the task must be awoken if [`Blocker::should_try`] may
    /// return `true` again.
    ///
    /// This must have Release ordering.
    ///
    /// This returns an ID which can be passed to [`Blocker::remove_task`] (on the same instance) to improve the
    /// performance of removal.
    fn prepare_to_wait(&self, task: &task::TaskRef);

    /// Remove a task from the wait queue of `self`. `id` is the value returned from [`Blocker::add_task`] when `task`
    /// was added.
    fn finish_wait(&self, task: &task::TaskRef);

    /// Block on self repeately until `cond` returns Some. This assumes that this blocker will be woken if `cond()`
    /// would change.
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
                    Task::current().block_on(&[self]);
                }
            };
        }
    }
}

static_assertions::assert_obj_safe!(Blocker);

impl CurrentTask {
    /// Wait for multiple blockers, waking if any wake.
    pub fn block_on<const N: usize>(&self, blockers: &[&dyn Blocker; N]) {
        {
            let mut state = self.lock.lock().unwrap();
            *state = TaskState::Blocking;
        }
        // TODO:PERFORMANCE: The need for these abort point checks is concerning, but seems hard to avoid. This need
        // investigation.
        CurrentServer::abort_point();
        for (i, blocker) in blockers.iter().enumerate() {
            blocker.prepare_to_wait(&self);
            if blocker.should_try() {
                for _ in 0..=i {
                    blockers[i].finish_wait(&self);
                }
                // We should try again and we have removed ourselves from all the task queues.
                return;
            }
        }
        self.park();
        CurrentServer::abort_point();
    }
}

#[derive(Default, Debug)]
pub struct TaskList {
    tasks: Mutex<Vec<task::TaskRef>>,
}

impl TaskList {
    pub(crate) fn add_task(&self, task: &task::TaskRef) {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks
            .iter()
            .all(|t| t.os_thread.id() != task.os_thread.id())
        {
            tasks.push(task.clone());
        }
    }

    pub(crate) fn remove_task(&self, task: &task::TaskRef) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(i) = tasks
            .iter()
            .position(|t| t.os_thread.id() == task.os_thread.id())
        {
            tasks.remove(i);
        }
    }

    pub(crate) fn wake_all(&self) {
        let tasks: Vec<task::TaskRef> = {
            let mut tasks = self.tasks.lock().unwrap();
            mem::take(tasks.as_mut())
        };
        for t in tasks {
            t.unpark();
        }
    }
}
