//! An implementation of kernel-thread-like task operations. It attempts to emulate the behavior of OS tasks, but will
//! be slow and not show the same race conditions. It is definitely not appropriate for fuzzing, but may be useful for
//! deterministic testing.

use core::{cell::Cell, marker::PhantomData, ops::Deref};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, LazyLock, Mutex},
    thread::{self, Thread, ThreadId},
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

/// A task which emulates the behavior of an OS task structure, but in userspace using the POSIX threading API.
#[derive(Debug)]
pub struct Task {
    pub(super) lock: Mutex<TaskState>,
    pub(super) cv: Condvar,
    pub(super) os_thread: Thread,
}

impl Task {
    pub fn current() -> CurrentTask {
        let mut map = TASK_MAP.lock().unwrap();
        let entry = map.entry(thread::current().id());
        let task = entry.or_insert_with(|| {
            Arc::new(Task {
                lock: Mutex::new(TaskState::Running),
                cv: Default::default(),
                os_thread: thread::current(),
            })
        });
        CurrentTask(task.clone(), PhantomData)
    }

    pub fn unpark(&self) {
        loop {
            let mut state = self.lock.lock().unwrap();
            // Loop until the task isn't "Locked". This weird way of doing it is to emulate the behavior of an OS
            // scheduler spinlock.
            if *state != TaskState::Blocking {
                *state = TaskState::Running;
                break;
            }
        }
        self.cv.notify_one();
    }
}

/// A reference to a Task which can be passed around. This is given a separate name to make porting code to a
/// non-reference counted form easier if that is required and to make it clear what the canonical way to reference a
/// task is.
pub type TaskRef = Arc<Task>;

static TASK_MAP: LazyLock<Mutex<HashMap<ThreadId, TaskRef>>> =
    LazyLock::new(|| Mutex::new(HashMap::<ThreadId, TaskRef>::new()));


/// An accessor for the currently executing task.
pub struct CurrentTask(TaskRef, PhantomData<&'static Cell<()>>);

static_assertions::assert_not_impl_any!(CurrentTask: Send, Sync);

impl std::ops::DerefMut for CurrentTask {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Deref for CurrentTask {
    type Target = TaskRef;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl CurrentTask {
    pub fn park(&self) {
        let mut state = self.0.lock.lock().unwrap();
        assert_eq!(*state, TaskState::Blocking);
        *state = TaskState::Blocked;
        while *state == TaskState::Blocked {
            state = self.cv.wait(state).unwrap();
        }
    }
}
