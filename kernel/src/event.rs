// SPDX-License-Identifier: MPL-2.0

use aster_time::Instant;
use ostd::task::Task;
use serde::Serialize;

use crate::{process::posix_thread::AsPosixThread as _, thread::Tid};

#[derive(Debug, Clone, Copy, Serialize)]
pub enum TaskId {
    KernelTask(usize),
    PosixThread(Tid),
    Unknown,
}

impl TaskId {
    pub fn new(task: &Task) -> Self {
        if let Some(t) = task.as_posix_thread() {
            Self::PosixThread(t.tid())
        } else {
            Self::KernelTask(task.id().into())
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct EventContext {
    pub task: TaskId,
    pub timestamp: Instant,
}

impl EventContext {
    /// Creates a new EventContext from the current context
    pub fn new() -> Self {
        EventContext {
            task: Task::current()
                .map(|t| TaskId::new(&t))
                .unwrap_or(TaskId::Unknown),
            timestamp: aster_time::read_monotonic_time().into(),
        }
    }
}
