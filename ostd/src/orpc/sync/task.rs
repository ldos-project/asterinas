//! An implementation of kernel-thread-like task operations. It attempts to emulate the behavior of OS tasks, but will
//! be slow and not show the same race conditions. It is definitely not appropriate for fuzzing, but may be useful for
//! deterministic testing.

use core::{
    cell::Cell,
    collections::HashMap,
    marker::PhantomData,
    ops::Deref,
    sync::{Arc, Condvar, LazyLock, Mutex},
    thread::{self, Thread, ThreadId},
};

use spin::Once;

use crate::{
    cpu::context::UserContext,
    prelude::*,
    sync::{Condvar, LazyLock, Mutex},
    trap::in_interrupt_context,
};

/// A reference to a Task which can be passed around. This is given a separate name to make porting code to a
/// non-reference counted form easier if that is required and to make it clear what the canonical way to reference a
/// task is.
pub type TaskRef = Arc<crate::task::Task>;

static TASK_MAP: Once<Mutex<HashMap<ThreadId, TaskRef>>> = Once::new();

pub fn init() {
    TASK_MAP.call_once(|| Mutex::new(HashMap::<ThreadId, TaskRef>::new()));
}

/// An accessor for the currently executing task.
pub struct CurrentTask(TaskRef, PhantomData<&'static Cell<()>>);

static_assertions::assert_not_impl_any!(CurrentTask: Send, Sync);

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
