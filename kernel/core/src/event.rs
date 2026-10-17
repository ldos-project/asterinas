// SPDX-License-Identifier: MPL-2.0

use aster_time::Instant;
use aster_util::fixed_str::FixedCStr;
use ostd::task::Task;
use ser::SerializeTupleVariant;
use serde::{
    Serialize, Serializer,
    ser::{self, SerializeTupleStruct},
};
use snafu::Location;

use crate::{process::posix_thread::AsPosixThread as _, thread::Tid};

#[derive(Clone, Copy, Debug)]
pub struct SerializableLocation(Location);

impl Serialize for SerializableLocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut tuple = serializer.serialize_tuple_struct("Location", 3)?;
        tuple.serialize_field(self.0.file)?;
        tuple.serialize_field(&self.0.line)?;
        tuple.serialize_field(&self.0.column)?;
        tuple.end()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TaskId {
    KernelTask(usize, Location),
    PosixThread(Tid, FixedCStr<16>),
    Unknown,
}

impl TaskId {
    pub fn new(task: &Task) -> Self {
        if let Some(t) = task.as_posix_thread() {
            Self::PosixThread(t.tid(), *t.thread_name().lock())
        } else {
            Self::KernelTask(task.id().into(), task.build_location())
        }
    }
}

impl Serialize for TaskId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            TaskId::KernelTask(id, location) => {
                // Serializer::serialize_tuple(self, len)
                let mut serde_state =
                    serializer.serialize_tuple_variant("TaskId", 0u32, "KernelTask", 2)?;
                serde_state.serialize_field(id)?;
                serde_state.serialize_field(&SerializableLocation(*location))?;
                serde_state.end()
            }
            TaskId::PosixThread(id, name) => {
                let mut serde_state =
                    serializer.serialize_tuple_variant("TaskId", 1u32, "PosixThread", 2)?;
                serde_state.serialize_field(id)?;
                serde_state.serialize_field(name)?;
                serde_state.end()
            }
            TaskId::Unknown => {
                Serializer::serialize_unit_variant(serializer, "TaskId", 2u32, "Unknown")
            }
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

impl Default for EventContext {
    fn default() -> Self {
        Self::new()
    }
}
