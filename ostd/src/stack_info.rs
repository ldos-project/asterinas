// SPDX-License-Identifier: MPL-2.0

//! Tools for capturing information about the stack execution context.

use alloc::{
    borrow::{Cow, ToOwned},
    fmt,
};
use core::{fmt::Display, num::NonZeroUsize};

use snafu::GenerateImplicitData;

use crate::{cpu::CpuId, stacktrace::CapturedStackTrace, task::Task};

/// A location in the code. Unlike, [`snafu::Location`] and [`core::panic::Location`], this can hold
/// a dynamic string. However, if given a `&'static str`, it will not allocate memory (internally it
/// uses [`Cow`]).
#[derive(Clone, Debug)]
pub struct Location {
    /// The file where the error was reported.
    ///
    /// This can be either a `&'static str` or a `String`.
    pub file: Cow<'static, str>,
    /// The line where the error was reported
    pub line: u32,
    /// The column where the error was reported
    pub column: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{file}:{line}:{column}",
            file = self.file,
            line = self.line,
            column = self.column,
        )
    }
}

impl From<&snafu::Location> for Location {
    fn from(value: &snafu::Location) -> Self {
        Self {
            file: value.file.into(),
            line: value.line,
            column: value.column,
        }
    }
}

impl From<&core::panic::Location<'static>> for Location {
    fn from(value: &core::panic::Location<'static>) -> Self {
        Self {
            file: value.file().into(),
            line: value.line(),
            column: value.column(),
        }
    }
}

impl Location {
    /// Create a [`Location`] from a [`core::panic::Location`] with a limited lifetime. This
    /// allocates memory in which to store the filename.
    pub fn from_location(value: &core::panic::Location<'_>) -> Self {
        Self {
            file: value.file().to_owned().into(),
            line: value.line(),
            column: value.column(),
        }
    }
}

/// The stack context in which an error may have occurred. This should be used in most error types.
///
/// This type may also be useful for capturing the context of some other event for use in later
/// errors or tracing. For example, a lock might capture the context every time it is locked, so
/// that `try_lock` errors can include the lock holder context along with `try_lock` call context.
/// This can be very useful for understanding deadlocks.
///
/// NOTE: This module has some degree of "premature" optimization. This is because we want to
/// encourage [`StackInfo`] to be used liberally so keeping the cost as low as possible has a
/// very significant value.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StackInfo {
    /// The stacktrace.
    pub stack_trace: CapturedStackTrace,
    /// The ID of the running [`Task`].
    pub task_id: Option<NonZeroUsize>,
    /// The ID of a CPU which was being executed on around the time the context was captured. This
    /// may or may not be the CPU on which the actual error occurred as a migration could happen
    /// during context capture.
    pub cpu_id: CpuId,
    /// The ID of the [`Server`](crate::orpc::framework::Server) context in which the error
    /// happened. This is the server that would have handled a panic at the point the error
    /// occurred.
    pub server_id: Option<NonZeroUsize>,
    /// The source location.
    pub location: Option<Location>,
}

impl StackInfo {
    /// Create a new `StackInfo`, dropping `skip` stackframes starting from the caller to this
    /// function.
    #[track_caller]
    pub fn new(skip: usize) -> StackInfo {
        let current_task = Task::current();
        let task_id = current_task.as_ref().map(|t| t.id());
        #[cfg(not(baseline_asterinas))]
        let server_id = current_task.and_then(|t| {
            t.server()
                .borrow()
                .as_ref()
                .map(|s| s.orpc_server_base().id())
        });
        #[cfg(baseline_asterinas)]
        let server_id = None;
        let stack_trace = CapturedStackTrace::capture(skip + 1);
        StackInfo {
            stack_trace,
            task_id,
            cpu_id: CpuId::current_racy(),
            server_id,
            location: Some(core::panic::Location::caller().into()),
        }
    }

    /// Create an "empty" [`StackInfo`] which contains all default information.
    pub const fn empty() -> Self {
        Self {
            stack_trace: CapturedStackTrace::empty(),
            task_id: None,
            cpu_id: CpuId::bsp(),
            server_id: None,
            location: None,
        }
    }
}

impl Display for StackInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(location) = &self.location {
            write!(f, "at {location} ")?;
        }
        if let Some(tid) = self.task_id {
            write!(f, "on task {tid} ")?;
        }
        if let Some(id) = self.server_id {
            write!(f, "in server {id} ")?;
        }
        write!(f, "on cpu {}", self.cpu_id.as_usize())?;
        write!(f, " with stack: {}", self.stack_trace)?;
        Ok(())
    }
}

impl GenerateImplicitData for StackInfo {
    #[track_caller]
    fn generate() -> Self {
        // We drop 2 frames: one for this call to `generate`, and one for the error constructor
        // (such as, a snafu generated `into_error` method).
        StackInfo::new(2)
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn stack_info_for_manual_checking() {
        fn fake_generated_function() -> StackInfo {
            StackInfo::generate()
        }
        let context = fake_generated_function();
        println!(
            r#"
Context for manual checking:
{}
The above line should be parsed by: 
cargo osdk enhance-log -b target/osdk/ostd/ostd-osdk-bin qemu.log 
and generate a stack trace with the top frame 
`ostd::stack_info::test::stack_info_for_manual_checking`.
"#,
            context
        );
    }

    #[ktest]
    fn stack_info_task_id() {
        let context = StackInfo::generate();
        let task = Task::current().unwrap();

        assert_eq!(context.task_id.unwrap(), task.id());
        assert!(context.server_id.is_none());
        assert_eq!(
            context.location.unwrap().file.as_ref(),
            core::panic::Location::caller().file()
        );
    }

    // There is no simple way to check the correctness of most of the values captured context. So we
    // settle for the test above that just performs the capture and checks what we can.
}
