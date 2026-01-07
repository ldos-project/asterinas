// SPDX-License-Identifier: MPL-2.0

//! Tools for capturing information about the stack execution context.

use core::{fmt::Display, num::NonZeroUsize, panic::Location};

use snafu::GenerateImplicitData;

use crate::{cpu::CpuId, stacktrace::CapturedStackTrace, task::Task};

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
#[derive(Clone, Copy, Debug)]
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
    ///
    /// NOTE: We use `core::panic::Location` instead of `snafu::Location`. `core::panic::Location`
    /// can be stored as a single pointer making this field 8-bytes instead of 24-bytes.
    pub location: Option<&'static Location<'static>>,
}

impl Display for StackInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(location) = self.location {
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
        let current_task = Task::current();
        let task_id = current_task.as_ref().map(|t| t.id());
        let server_id = current_task.and_then(|t| {
            t.server()
                .borrow()
                .as_ref()
                .map(|s| s.orpc_server_base().id())
        });
        // We drop 2 frames: one for this call to `generate`, and one for the error constructor
        // (such as, a snafu generated `into_error` method).
        let stack_trace = CapturedStackTrace::capture(2);
        StackInfo {
            stack_trace,
            task_id,
            cpu_id: CpuId::current_racy(),
            server_id,
            location: Some(Location::caller()),
        }
    }
}

#[cfg(ktest)]
mod test {
    use alloc::borrow::ToOwned;

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
            context.location.unwrap().file().to_owned(),
            Location::caller().file().to_owned()
        );
    }

    // There is no simple way to check the correctness of most of the values captured context. So we
    // settle for the test above that just performs the capture and checks what we can.
}
