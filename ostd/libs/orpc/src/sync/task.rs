//! An implementation of kernel-thread-like task operations. It attempts to emulate the behavior of OS tasks, but will
//! be slow and not show the same race conditions. It is definitely not appropriate for fuzzing, but may be useful for
//! deterministic testing.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    cell::Cell,
    clone::Clone,
    cmp::{Eq, PartialEq},
    default::Default,
    fmt::Debug,
    marker::Copy,
    mem,
    ops::Deref,
    prelude::rust_2024::derive,
};

pub use orpc_macros::select;

use crate::{
    orpc_impl::framework::CurrentServer,
    sync::mutex::{Mutex, MutexImpl},
};

/// The state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskState {
    /// The task is currently running or could be running.
    Running,
    /// The task is in the process of transitioning to `Blocked`. This state occurs while checking the conditions for a
    /// blocking operation.
    Blocking,
    /// The task is currently blocked waiting to be explicitly awakened.
    Blocked,
}

pub trait Task {
    fn current(&self) -> Box<dyn CurrentTask>;
    fn unpark(&self);
}

/// A reference to a Task which can be passed around. This is given a separate name to make porting code to a
/// non-reference counted form easier if that is required and to make it clear what the canonical way to reference a
/// task is.
pub type TaskRef = Arc<dyn Task>;

// A trait [`Blocker`] which allows a thread to wait for a wake-up from another thread. The API is designed to allow a
// waiter to wait on multiple blockers at the same time to support [`select!`].
//
// XXX: This needs to be reworked significantly because is forces some rather odd syntax in select and is not as
// flexible as it should be.

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
    fn prepare_to_wait(&self, task: &TaskRef);

    /// Remove a task from the wait queue of `self`. `id` is the value returned from [`Blocker::add_task`] when `task`
    /// was added.
    fn finish_wait(&self, task: &TaskRef);

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

#[derive(Default, Debug)]
pub struct TaskList<M: MutexImpl<Vec<TaskRef>>> {
    tasks: Mutex<Vec<TaskRef>, M>,
}

impl<M: MutexImpl<Vec<TaskRef>>> TaskList<M> {
    pub(crate) fn add_task(&self, task: &TaskRef) {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks
            .iter()
            .all(|t| t.os_thread.id() != task.os_thread.id())
        {
            tasks.push(task.clone());
        }
    }

    pub(crate) fn remove_task(&self, task: &TaskRef) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(i) = tasks
            .iter()
            .position(|t| t.os_thread.id() == task.os_thread.id())
        {
            tasks.remove(i);
        }
    }

    pub(crate) fn wake_all(&self) {
        let tasks: Vec<TaskRef> = {
            let mut tasks = self.tasks.lock().unwrap();
            mem::take(tasks.as_mut())
        };
        for t in tasks {
            t.unpark();
        }
    }
}

/// An accessor for the currently executing task.
pub trait CurrentTask {
    fn park(&self);
    /// Wait for multiple blockers, waking if any wake.
}


pub fn block_on<const N: usize>(current: &CurrentTask, blockers: &[&dyn Blocker; N]) {
    }
